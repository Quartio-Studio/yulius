//! Automations: cron-scheduled task creation with deduplicated runs (SPEC-001 AUT-001..005,
//! SPEC-002 §6.4, AC-AUT-1/2/4).
//!
//! Run lifecycle: `scheduled → task_created | skipped_previous_active | skipped_disabled |
//! failed`. The scheduler inserts the run row (unique per automation and scheduled time)
//! **before** it creates the task, and the task carries the run key `<automation>@<ms>`
//! (unique in the store). After a crash, rows still `scheduled` are resolved once at startup by
//! linking the task created with that key or creating it; a racing second creation fails on the
//! unique key and links instead, so one scheduled time never yields two tasks.
//!
//! Timing: [`App::automation_startup`] runs once (crash recovery plus the single catch-up run of
//! [`Schedule::catch_up`]); [`App::automation_tick`] runs every [`SCHEDULER_INTERVAL_MS`] and
//! takes the due slot from [`Schedule::due_runs`]. When more than one slot elapsed since the last
//! handled one (the process was suspended), the tick applies the same rule as the catch-up: the
//! stale slots are skipped and the automation resumes at its next regular time. Each automation
//! has an anchor (the last handled time); enabling it or changing its schedule moves the anchor
//! to "now" so no history is replayed.

pub mod templates;

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use yulius_contracts::api::automations::{Automation, AutomationRunView, AutomationTemplate};
use yulius_contracts::events::UiEvent;
use yulius_domain::cron::{MAX_UTC_OFFSET_MINUTES, Schedule};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::permission::Action;
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::error::{AppError, AppResult};
use crate::lifecycle::{MAX_PROMPT_BYTES, NewTask};
use crate::model::{TaskOrigin, TaskRecord};

/// Scheduler period.
pub const SCHEDULER_INTERVAL_MS: u64 = 30_000;
/// Most automations per sandbox.
pub const MAX_AUTOMATIONS_PER_SANDBOX: u32 = 100;
/// Longest automation name.
pub const MAX_NAME_CHARS: usize = 100;
/// Crash-left runs resolved per startup.
pub const MAX_PENDING_PER_PASS: u32 = 200;
/// Runs returned by the history endpoint.
pub const RUN_HISTORY_LIMIT: u32 = 100;

/// How an automation was created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CreatedVia {
    /// Web UI or API.
    Ui,
    /// An agent through the Yulius MCP tool (AUT-002).
    Agent,
}

impl CreatedVia {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Agent => "agent",
        }
    }

    /// Parses a wire name (unknown → `Ui`).
    #[must_use]
    pub fn parse(text: &str) -> Self {
        if text == "agent" {
            Self::Agent
        } else {
            Self::Ui
        }
    }
}

/// State of one automation run (SPEC-002 §6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunState {
    /// Row inserted; the task is being created (or a crash interrupted creation).
    Scheduled,
    /// The task exists.
    TaskCreated,
    /// The automation's previous task was still active.
    SkippedPreviousActive,
    /// The automation was disabled when the slot was processed.
    SkippedDisabled,
    /// Task creation failed (see the error).
    Failed,
}

impl RunState {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::TaskCreated => "task_created",
            Self::SkippedPreviousActive => "skipped_previous_active",
            Self::SkippedDisabled => "skipped_disabled",
            Self::Failed => "failed",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "scheduled" => Some(Self::Scheduled),
            "task_created" => Some(Self::TaskCreated),
            "skipped_previous_active" => Some(Self::SkippedPreviousActive),
            "skipped_disabled" => Some(Self::SkippedDisabled),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// A stored automation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRecord {
    /// Opaque id.
    pub id: String,
    /// Sandbox whose tasks it creates.
    pub sandbox_id: SandboxId,
    /// Name (also the created tasks' title).
    pub name: String,
    /// Five-field cron expression (validated by [`Schedule::parse`]).
    pub cron: String,
    /// Fixed UTC offset used to evaluate the schedule.
    pub utc_offset_minutes: i32,
    /// Prompt of each created task.
    pub prompt: String,
    /// Workflow of created tasks.
    pub workflow_id: Option<String>,
    /// Harness of created tasks.
    pub harness: Harness,
    /// Enabled.
    pub enabled: bool,
    /// User the created tasks are attributed to.
    pub created_by: UserId,
    /// UI or agent.
    pub created_via: CreatedVia,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last handled schedule time (anchor); `None` = creation time.
    pub last_scheduled: Option<Timestamp>,
}

impl AutomationRecord {
    /// The schedule anchor: last handled time or creation time.
    #[must_use]
    pub fn anchor(&self) -> Timestamp {
        self.last_scheduled.unwrap_or(self.created_at)
    }
}

/// One automation run row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRunRecord {
    /// Automation.
    pub automation_id: String,
    /// Scheduled time (with the automation, the idempotency key).
    pub scheduled_for: Timestamp,
    /// State.
    pub state: RunState,
    /// Created or linked task.
    pub task_id: Option<TaskId>,
    /// Failure or skip explanation.
    pub error: Option<String>,
    /// Row creation time.
    pub created_at: Timestamp,
}

/// An audit-log entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    /// Time.
    pub at: Timestamp,
    /// Actor (`<login>`, `task:<n>`, `system`).
    pub actor: String,
    /// Action.
    pub action: String,
    /// Detail.
    pub detail: Option<String>,
}

/// Counts of one scheduler pass, by final run state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SchedulerReport {
    /// Runs ending `task_created` (new or linked).
    pub created: u32,
    /// Runs ending `skipped_*`.
    pub skipped: u32,
    /// Runs ending `failed`.
    pub failed: u32,
    /// Automations whose stale slots were dropped (more than one elapsed slot).
    pub stale: u32,
}

impl SchedulerReport {
    fn count(&mut self, run: &AutomationRunRecord) {
        match run.state {
            RunState::TaskCreated => self.created += 1,
            RunState::SkippedPreviousActive | RunState::SkippedDisabled => self.skipped += 1,
            RunState::Failed => self.failed += 1,
            RunState::Scheduled => {}
        }
    }
}

/// The key stored on tasks created by a run.
#[must_use]
pub fn run_key(automation: &str, scheduled_for: Timestamp) -> String {
    format!("{automation}@{}", scheduled_for.0)
}

/// API view of a run.
#[must_use]
pub fn run_view(run: &AutomationRunRecord) -> AutomationRunView {
    AutomationRunView {
        scheduled_for: rfc3339(run.scheduled_for),
        state: run.state.as_str().to_owned(),
        task_id: run.task_id.as_ref().map(ToString::to_string),
        error: run.error.clone(),
    }
}

/// Validated editable fields.
struct Fields {
    name: String,
    cron: String,
    utc_offset_minutes: i32,
    prompt: String,
    harness: Harness,
}

/// Everything needed to insert an automation.
struct NewAutomation {
    sandbox: SandboxId,
    fields: Fields,
    workflow_id: Option<String>,
    enabled: bool,
    created_by: UserId,
    created_via: CreatedVia,
    actor: String,
    detail: Value,
}

fn validate_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::Invalid(format!(
            "the automation name must be 1 to {MAX_NAME_CHARS} characters"
        )));
    }
    Ok(name.to_owned())
}

fn validate_cron(cron: &str) -> AppResult<String> {
    let schedule = Schedule::parse(cron.trim()).map_err(|e| AppError::Invalid(e.to_string()))?;
    Ok(schedule.as_str().to_owned())
}

fn validate_offset(minutes: i64) -> AppResult<i32> {
    i32::try_from(minutes)
        .ok()
        .filter(|m| m.abs() <= MAX_UTC_OFFSET_MINUTES)
        .ok_or_else(|| {
            AppError::Invalid(format!(
                "utc_offset_minutes must be between -{MAX_UTC_OFFSET_MINUTES} and {MAX_UTC_OFFSET_MINUTES}"
            ))
        })
}

fn validate_prompt(prompt: &str) -> AppResult<String> {
    let prompt = prompt.trim();
    if prompt.is_empty() || prompt.len() > MAX_PROMPT_BYTES {
        return Err(AppError::Invalid(
            "the prompt must be 1 byte to 64 KiB".into(),
        ));
    }
    Ok(prompt.to_owned())
}

fn validate_harness(text: &str) -> AppResult<Harness> {
    Harness::parse(text).map_err(|_| {
        AppError::Invalid("harness must be claude (Yulius runs Claude Code only)".into())
    })
}

fn validate(input: &Automation) -> AppResult<Fields> {
    Ok(Fields {
        name: validate_name(&input.name)?,
        cron: validate_cron(&input.cron)?,
        utc_offset_minutes: validate_offset(i64::from(input.utc_offset_minutes))?,
        prompt: validate_prompt(&input.prompt)?,
        harness: validate_harness(&input.harness)?,
    })
}

fn str_arg<'a>(args: &'a Value, key: &str) -> AppResult<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Invalid(format!("missing string argument `{key}`")))
}

impl App {
    /// API view of an automation (next run and last run included).
    ///
    /// # Errors
    /// Store failures.
    pub async fn automation_view(&self, record: &AutomationRecord) -> AppResult<Automation> {
        let last = self.deps.store.runs(&record.id, 1).await?;
        let next_run_at = if record.enabled {
            Schedule::parse(&record.cron)
                .ok()
                .and_then(|s| s.next_after(self.now(), record.utc_offset_minutes))
                .map(rfc3339)
        } else {
            None
        };
        Ok(Automation {
            id: record.id.clone(),
            name: record.name.clone(),
            cron: record.cron.clone(),
            utc_offset_minutes: record.utc_offset_minutes,
            prompt: record.prompt.clone(),
            workflow_id: record.workflow_id.clone(),
            harness: record.harness.as_str().to_owned(),
            enabled: record.enabled,
            created_via: record.created_via.as_str().to_owned(),
            next_run_at,
            last_run: last.first().map(run_view),
        })
    }

    async fn resolve_workflow_id(
        &self,
        sandbox: &SandboxId,
        workflow: Option<&str>,
    ) -> AppResult<Option<String>> {
        match workflow.map(str::trim).filter(|w| !w.is_empty()) {
            None => Ok(None),
            Some(id) => Ok(Some(self.visible_workflow(sandbox, id).await?.id)),
        }
    }

    /// `GET /api/sandboxes/{sid}/automations`.
    ///
    /// # Errors
    /// `NotFound` without access.
    pub async fn list_automations(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
    ) -> AppResult<Vec<Automation>> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::View).await?;
        let mut out = Vec::new();
        for record in self.deps.store.automations(&sid).await? {
            out.push(self.automation_view(&record).await?);
        }
        Ok(out)
    }

    async fn insert_automation(&self, new: NewAutomation) -> AppResult<AutomationRecord> {
        let NewAutomation {
            sandbox,
            fields,
            workflow_id,
            enabled,
            created_by,
            created_via,
            actor,
            detail,
        } = new;
        let now = self.now();
        let record = AutomationRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: sandbox,
            name: fields.name,
            cron: fields.cron,
            utc_offset_minutes: fields.utc_offset_minutes,
            prompt: fields.prompt,
            workflow_id,
            harness: fields.harness,
            enabled,
            created_by,
            created_via,
            created_at: now,
            last_scheduled: Some(now),
        };
        self.deps
            .store
            .insert_automation(&record, MAX_AUTOMATIONS_PER_SANDBOX)
            .await?;
        let mut detail = detail;
        detail["created_via"] = json!(created_via.as_str());
        detail["name"] = json!(record.name);
        detail["cron"] = json!(record.cron);
        self.deps
            .store
            .audit(
                now,
                &actor,
                "automation.created",
                Some(&record.id),
                Some(&detail.to_string()),
            )
            .await?;
        Ok(record)
    }

    /// `POST /api/sandboxes/{sid}/automations` (write access).
    ///
    /// # Errors
    /// Access, validation and capacity errors.
    pub async fn create_automation(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        input: &Automation,
    ) -> AppResult<Automation> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::Configure).await?;
        let fields = validate(input)?;
        let workflow = self
            .resolve_workflow_id(&sid, input.workflow_id.as_deref())
            .await?;
        let record = self
            .insert_automation(NewAutomation {
                sandbox: sid,
                fields,
                workflow_id: workflow,
                enabled: input.enabled,
                created_by: ctx.user.id.clone(),
                created_via: CreatedVia::Ui,
                actor: self.login_of(&ctx.user.id).await,
                detail: json!({}),
            })
            .await?;
        self.automation_view(&record).await
    }

    /// `GET /api/sandboxes/{sid}/automation-templates`: the ready-made automations (SPEC-042
    /// AUT-008), each with the project's automation of the same name, if any.
    ///
    /// # Errors
    /// `NotFound` without access.
    pub async fn list_automation_templates(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
    ) -> AppResult<Vec<AutomationTemplate>> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::View).await?;
        let existing = self.deps.store.automations(&sid).await?;
        Ok(templates::templates()
            .iter()
            .map(|t| AutomationTemplate {
                id: t.id.clone(),
                name: t.name.clone(),
                description: t.description.clone(),
                cron: t.cron.clone(),
                utc_offset_minutes: t.utc_offset_minutes,
                prompt: t.prompt.clone(),
                workflow_id: t.workflow.clone(),
                automation_id: existing
                    .iter()
                    .find(|a| templates::same_name(t, &a.name))
                    .map(|a| a.id.clone()),
            })
            .collect())
    }

    /// `POST /api/sandboxes/{sid}/automation-templates/{id}/enable` (write access): creates the
    /// ready-made automation, enabled and attributed to the caller (`true`), or enables the
    /// project's automation of the same name, keeping its content (`false`).
    ///
    /// # Errors
    /// `NotFound` for an unknown template or without access; access and capacity errors.
    pub async fn enable_automation_template(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        template: &str,
    ) -> AppResult<(Automation, bool)> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::Configure).await?;
        let actor = self.login_of(&ctx.user.id).await;
        self.enable_template_as(
            &sid,
            template,
            &ctx.user.id,
            CreatedVia::Ui,
            &actor,
            json!({}),
        )
        .await
    }

    /// [`App::enable_automation_template`] once access is checked: on behalf of `created_by`,
    /// audited as `actor` with `detail` and the template (`automation_template_enable`, SPEC-066
    /// AUT-012, passes its caller and `created_via = agent`).
    ///
    /// # Errors
    /// `NotFound` for an unknown template; capacity errors.
    pub(crate) async fn enable_template_as(
        &self,
        sid: &SandboxId,
        template: &str,
        created_by: &UserId,
        created_via: CreatedVia,
        actor: &str,
        detail: Value,
    ) -> AppResult<(Automation, bool)> {
        let template = templates::template(template).ok_or(AppError::NotFound)?;
        let mut detail = detail;
        detail["template"] = json!(template.id);
        // The name check and the insert are one step: Yulius is one process (ADR-0001), and
        // enables are rare, so one instance-wide lock is enough.
        let _serial = self.automation_template_lock.lock().await;
        let existing = self
            .deps
            .store
            .automations(sid)
            .await?
            .into_iter()
            .find(|a| templates::same_name(template, &a.name));
        if let Some(current) = existing {
            if !current.enabled {
                let next = AutomationRecord {
                    enabled: true,
                    ..current.clone()
                };
                self.save_automation(&current, &next, actor, detail).await?;
            }
            let stored = self
                .deps
                .store
                .automation(&current.id)
                .await?
                .ok_or(AppError::NotFound)?;
            return Ok((self.automation_view(&stored).await?, false));
        }
        let fields = Fields {
            name: validate_name(&template.name)?,
            cron: validate_cron(&template.cron)?,
            utc_offset_minutes: validate_offset(i64::from(template.utc_offset_minutes))?,
            prompt: validate_prompt(&template.prompt)?,
            harness: Harness::ClaudeCode,
        };
        let workflow = self
            .resolve_workflow_id(sid, template.workflow.as_deref())
            .await?;
        let record = self
            .insert_automation(NewAutomation {
                sandbox: sid.clone(),
                fields,
                workflow_id: workflow,
                enabled: true,
                created_by: created_by.clone(),
                created_via,
                actor: actor.to_owned(),
                detail,
            })
            .await?;
        Ok((self.automation_view(&record).await?, true))
    }

    async fn automation_for_change(
        &self,
        ctx: &AuthContext,
        id: &str,
        action: Action,
    ) -> AppResult<AutomationRecord> {
        let record = self
            .deps
            .store
            .automation(id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.require(&ctx.user, &record.sandbox_id, action).await?;
        Ok(record)
    }

    /// Stores changed fields; moves the anchor to now when the automation becomes enabled or
    /// its schedule changes, so missed history is never replayed.
    async fn save_automation(
        &self,
        current: &AutomationRecord,
        next: &AutomationRecord,
        actor: &str,
        detail: Value,
    ) -> AppResult<()> {
        if !self.deps.store.update_automation(next).await? {
            return Err(AppError::NotFound);
        }
        let rescheduled = (next.enabled && !current.enabled)
            || next.cron != current.cron
            || next.utc_offset_minutes != current.utc_offset_minutes;
        if rescheduled {
            self.deps
                .store
                .advance_automation_anchor(&next.id, self.now())
                .await?;
        }
        let mut detail = detail;
        detail["enabled"] = json!(next.enabled);
        detail["cron"] = json!(next.cron);
        self.deps
            .store
            .audit(
                self.now(),
                actor,
                "automation.updated",
                Some(&next.id),
                Some(&detail.to_string()),
            )
            .await
    }

    /// `PUT /api/automations/{id}`: replaces the editable fields.
    ///
    /// # Errors
    /// Access and validation errors.
    pub async fn update_automation(
        &self,
        ctx: &AuthContext,
        id: &str,
        input: &Automation,
    ) -> AppResult<Automation> {
        let current = self
            .automation_for_change(ctx, id, Action::Configure)
            .await?;
        let fields = validate(input)?;
        let workflow_id = self
            .resolve_workflow_id(&current.sandbox_id, input.workflow_id.as_deref())
            .await?;
        let next = AutomationRecord {
            name: fields.name,
            cron: fields.cron,
            utc_offset_minutes: fields.utc_offset_minutes,
            prompt: fields.prompt,
            harness: fields.harness,
            workflow_id,
            enabled: input.enabled,
            ..current.clone()
        };
        let actor = self.login_of(&ctx.user.id).await;
        self.save_automation(&current, &next, &actor, json!({}))
            .await?;
        let stored = self
            .deps
            .store
            .automation(id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.automation_view(&stored).await
    }

    /// `DELETE /api/automations/{id}`: removes the automation and its run history; created
    /// tasks stay.
    ///
    /// # Errors
    /// Access errors.
    pub async fn delete_automation(&self, ctx: &AuthContext, id: &str) -> AppResult<()> {
        let actor = self.login_of(&ctx.user.id).await;
        self.delete_automation_as(ctx, id, &actor, None).await
    }

    /// [`App::delete_automation`] audited as `actor`; a management tool passes its caller and
    /// the caller's audit detail (SPEC-066 MGT-010).
    ///
    /// # Errors
    /// Access errors.
    pub(crate) async fn delete_automation_as(
        &self,
        ctx: &AuthContext,
        id: &str,
        actor: &str,
        caller_detail: Option<&Value>,
    ) -> AppResult<()> {
        let record = self
            .automation_for_change(ctx, id, Action::Configure)
            .await?;
        if !self.deps.store.delete_automation(&record.id).await? {
            return Err(AppError::NotFound);
        }
        let detail = match caller_detail {
            Some(caller) => {
                let mut detail = caller.clone();
                detail["name"] = json!(record.name);
                detail.to_string()
            }
            None => record.name.clone(),
        };
        self.deps
            .store
            .audit(
                self.now(),
                actor,
                "automation.deleted",
                Some(&record.id),
                Some(&detail),
            )
            .await
    }

    /// `GET /api/automations/{id}/runs`: newest first.
    ///
    /// # Errors
    /// Access errors.
    pub async fn automation_runs(
        &self,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<Vec<AutomationRunView>> {
        let record = self.automation_for_change(ctx, id, Action::View).await?;
        Ok(self
            .deps
            .store
            .runs(&record.id, RUN_HISTORY_LIMIT)
            .await?
            .iter()
            .map(run_view)
            .collect())
    }

    /// `POST /api/automations/{id}/run`: one run now (also for disabled automations; the
    /// previous-active rule still applies).
    ///
    /// # Errors
    /// Access errors and store failures.
    pub async fn run_automation_now(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<AutomationRunView> {
        let actor = self.login_of(&ctx.user.id).await;
        self.run_automation_now_as(ctx, id, &actor, None).await
    }

    /// [`App::run_automation_now`] audited as `actor`; a management tool passes its caller and
    /// the caller's audit detail (SPEC-066 MGT-010).
    ///
    /// # Errors
    /// Access errors and store failures.
    pub(crate) async fn run_automation_now_as(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        actor: &str,
        caller_detail: Option<&Value>,
    ) -> AppResult<AutomationRunView> {
        let record = self
            .automation_for_change(ctx, id, Action::Configure)
            .await?;
        let slot = self.now();
        let run = self.process_slot(&record, slot, true).await?;
        let detail = match caller_detail {
            Some(caller) => {
                let mut detail = caller.clone();
                detail["state"] = json!(run.state.as_str());
                detail.to_string()
            }
            None => run.state.as_str().to_owned(),
        };
        self.deps
            .store
            .audit(
                self.now(),
                actor,
                "automation.run_now",
                Some(&record.id),
                Some(&detail),
            )
            .await?;
        Ok(run_view(&run))
    }

    /// Starts the scheduler: one [`App::automation_startup`] pass, then
    /// [`App::automation_tick`] every [`SCHEDULER_INTERVAL_MS`]. Abort the returned handle to
    /// stop it (each pass is bounded by its store calls and task creation).
    pub fn start_automation_scheduler(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let app = Arc::clone(self);
        tokio::spawn(async move {
            match app.automation_startup().await {
                Ok(report) => tracing::info!(?report, "automation scheduler started"),
                Err(err) => tracing::warn!(error = %err, "automation startup pass failed"),
            }
            let mut interval = tokio::time::interval(Duration::from_millis(SCHEDULER_INTERVAL_MS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                interval.tick().await;
                if let Err(err) = app.automation_tick().await {
                    tracing::warn!(error = %err, "automation tick failed");
                }
            }
        })
    }

    /// Startup pass: resolves runs left `scheduled` by a crash, then gives every enabled
    /// automation at most one catch-up run ([`Schedule::catch_up`]) and moves its anchor to now.
    ///
    /// # Errors
    /// Store failures listing automations or runs (single-run failures are recorded on the
    /// run instead).
    pub async fn automation_startup(self: &Arc<Self>) -> AppResult<SchedulerReport> {
        let store = &self.deps.store;
        let mut report = SchedulerReport::default();
        for pending in store.pending_runs(MAX_PENDING_PER_PASS).await? {
            let Some(automation) = store.automation(&pending.automation_id).await? else {
                continue;
            };
            match self
                .resolve_pending(&automation, pending.scheduled_for)
                .await
            {
                Ok(run) => report.count(&run),
                Err(err) => {
                    tracing::warn!(error = %err, automation = %automation.id, "pending run not resolved");
                }
            }
        }
        let now = self.now();
        for automation in store.enabled_automations().await? {
            let Ok(schedule) = Schedule::parse(&automation.cron) else {
                tracing::warn!(automation = %automation.id, "stored cron expression is invalid");
                continue;
            };
            if let Some(slot) =
                schedule.catch_up(automation.anchor(), now, automation.utc_offset_minutes)
            {
                match self.process_slot(&automation, slot, false).await {
                    Ok(run) => report.count(&run),
                    Err(err) => {
                        tracing::warn!(error = %err, automation = %automation.id, "catch-up run failed");
                        continue;
                    }
                }
            }
            store.advance_automation_anchor(&automation.id, now).await?;
        }
        Ok(report)
    }

    /// Periodic pass: processes the slot that became due since each enabled automation's
    /// anchor. More than one elapsed slot means the scheduler did not run in between; those
    /// stale slots are dropped like missed runs at startup.
    ///
    /// # Errors
    /// Store failures listing automations.
    pub async fn automation_tick(self: &Arc<Self>) -> AppResult<SchedulerReport> {
        let store = &self.deps.store;
        let mut report = SchedulerReport::default();
        let now = self.now();
        for automation in store.enabled_automations().await? {
            let Ok(schedule) = Schedule::parse(&automation.cron) else {
                tracing::warn!(automation = %automation.id, "stored cron expression is invalid");
                continue;
            };
            let due = schedule.due_runs(
                Some(automation.anchor()),
                now,
                automation.utc_offset_minutes,
                2,
            );
            match due.as_slice() {
                [] => {}
                [slot] => match self.process_slot(&automation, *slot, false).await {
                    Ok(run) => {
                        report.count(&run);
                        store
                            .advance_automation_anchor(&automation.id, *slot)
                            .await?;
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, automation = %automation.id, "scheduled run failed; retrying next tick");
                    }
                },
                _ => {
                    tracing::info!(automation = %automation.id, "several runs elapsed while the scheduler was not running; resuming at the next regular time");
                    report.stale += 1;
                    store.advance_automation_anchor(&automation.id, now).await?;
                }
            }
        }
        Ok(report)
    }

    async fn previous_active_task(
        &self,
        automation: &AutomationRecord,
    ) -> AppResult<Option<TaskRecord>> {
        let store = &self.deps.store;
        let Some(run) = store.last_task_run(&automation.id).await? else {
            return Ok(None);
        };
        let Some(task_id) = run.task_id else {
            return Ok(None);
        };
        Ok(store
            .task(&task_id)
            .await?
            .filter(|t| Self::is_active_status(t.status)))
    }

    /// Handles one slot: dedupe on the run row, skip rules, then task creation.
    async fn process_slot(
        self: &Arc<Self>,
        automation: &AutomationRecord,
        slot: Timestamp,
        manual: bool,
    ) -> AppResult<AutomationRunRecord> {
        let store = &self.deps.store;
        if let Some(existing) = store.automation_run(&automation.id, slot).await? {
            return if existing.state == RunState::Scheduled {
                self.resolve_pending(automation, slot).await
            } else {
                Ok(existing)
            };
        }
        let fresh = store
            .automation(&automation.id)
            .await?
            .ok_or(AppError::NotFound)?;
        let skip = if !manual && !fresh.enabled {
            Some((
                RunState::SkippedDisabled,
                "the automation is disabled".to_owned(),
            ))
        } else {
            self.previous_active_task(&fresh).await?.map(|t| {
                (
                    RunState::SkippedPreviousActive,
                    format!(
                        "the previous task #{} is still {}",
                        t.number,
                        t.status.as_str()
                    ),
                )
            })
        };
        let now = self.now();
        let (state, error) = match skip {
            Some((state, reason)) => (state, Some(reason)),
            None => (RunState::Scheduled, None),
        };
        let row = AutomationRunRecord {
            automation_id: fresh.id.clone(),
            scheduled_for: slot,
            state,
            task_id: None,
            error,
            created_at: now,
        };
        if !store.insert_run(&row).await? {
            // Another pass inserted the same slot first; follow its outcome.
            let existing = store
                .automation_run(&fresh.id, slot)
                .await?
                .ok_or(AppError::NotFound)?;
            return if existing.state == RunState::Scheduled {
                self.resolve_pending(&fresh, slot).await
            } else {
                Ok(existing)
            };
        }
        if state != RunState::Scheduled {
            self.announce_run(&fresh, &row).await;
            return Ok(row);
        }
        self.resolve_pending(&fresh, slot).await
    }

    /// Completes a `scheduled` run: links the task created with its key, or creates it.
    async fn resolve_pending(
        self: &Arc<Self>,
        automation: &AutomationRecord,
        slot: Timestamp,
    ) -> AppResult<AutomationRunRecord> {
        let store = &self.deps.store;
        let key = run_key(&automation.id, slot);
        let outcome: Result<TaskId, AppError> = match store.task_by_automation_run(&key).await? {
            Some(task) => Ok(task),
            None => match self.create_automation_task(automation, &key).await {
                Ok(task) => Ok(task),
                Err(err) => match store.task_by_automation_run(&key).await? {
                    Some(task) => Ok(task),
                    None => Err(err),
                },
            },
        };
        let (state, task, error) = match outcome {
            Ok(task) => (RunState::TaskCreated, Some(task), None),
            Err(err) => (RunState::Failed, None, Some(err.public_message())),
        };
        let finished = store
            .finish_run(&automation.id, slot, state, task.as_ref(), error.as_deref())
            .await?;
        let run = store
            .automation_run(&automation.id, slot)
            .await?
            .ok_or(AppError::NotFound)?;
        if finished {
            self.announce_run(automation, &run).await;
        }
        Ok(run)
    }

    async fn create_automation_task(
        self: &Arc<Self>,
        automation: &AutomationRecord,
        key: &str,
    ) -> AppResult<TaskId> {
        let steps = self
            .workflow_steps(&automation.sandbox_id, automation.workflow_id.as_deref())
            .await?;
        let task = self
            .create_task_internal(NewTask {
                sandbox: automation.sandbox_id.clone(),
                created_by: automation.created_by.clone(),
                prompt: automation.prompt.clone(),
                harness: automation.harness,
                title: Some(automation.name.clone()),
                workflow_id: automation.workflow_id.clone(),
                steps,
                model: None,
                effort: None,
                origin: TaskOrigin::Automation,
                automation_run: Some(key.to_owned()),
                start: true,
                mode: crate::model::TaskMode::Prompt,
            })
            .await?;
        Ok(task.id)
    }

    async fn announce_run(&self, automation: &AutomationRecord, run: &AutomationRunRecord) {
        self.events.publish(
            Some(automation.sandbox_id.clone()),
            false,
            UiEvent::AutomationRun {
                automation_id: automation.id.clone(),
                state: run.state.as_str().to_owned(),
                task_id: run.task_id.as_ref().map(ToString::to_string),
            },
        );
        let detail = json!({
            "scheduled_for": run.scheduled_for.0,
            "state": run.state.as_str(),
            "task_id": run.task_id.as_ref().map(TaskId::as_str),
            "error": run.error,
        });
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                "system",
                "automation.run",
                Some(&automation.id),
                Some(&detail.to_string()),
            )
            .await
        {
            tracing::warn!(error = %err, "automation run audit failed");
        }
    }

    pub(crate) async fn agent_automation(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<AutomationRecord> {
        let id = str_arg(args, "automation_id")?;
        self.deps
            .store
            .automation(id)
            .await?
            .filter(|a| a.sandbox_id == caller.sandbox)
            .ok_or(AppError::NotFound)
    }

    pub(crate) async fn agent_workflow(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Option<String>> {
        let Some(wanted) = args
            .get("workflow")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|w| !w.is_empty())
        else {
            return Ok(None);
        };
        let workflows = self.workflows_for(&caller.sandbox).await?;
        workflows
            .iter()
            .find(|w| w.id == wanted)
            .or_else(|| {
                workflows
                    .iter()
                    .find(|w| w.name.eq_ignore_ascii_case(wanted))
            })
            .map(|w| Some(w.id.clone()))
            .ok_or_else(|| {
                AppError::Invalid(format!(
                    "unknown workflow `{wanted}`; use workflows_list to see the available ones"
                ))
            })
    }

    /// `automation_create` agent tool (AUT-002): records `created_via = agent` and the caller
    /// (the agent's task or the Cockpit) in the audit log; tasks are attributed to the person the
    /// caller acts for (SPEC-037 MGT-007).
    ///
    /// # Errors
    /// Validation and capacity errors.
    pub async fn tool_automation_create(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        self.manage_authority(caller).await?;
        let offset = match args.get("utc_offset_minutes") {
            None | Some(Value::Null) => 0,
            Some(v) => v
                .as_i64()
                .ok_or_else(|| AppError::Invalid("utc_offset_minutes must be an integer".into()))?,
        };
        let harness = match args.get("harness").and_then(Value::as_str) {
            Some(h) => validate_harness(h)?,
            None => caller
                .task
                .as_ref()
                .map_or(Harness::ClaudeCode, |t| t.harness),
        };
        let fields = Fields {
            name: validate_name(str_arg(args, "name")?)?,
            cron: validate_cron(str_arg(args, "cron")?)?,
            utc_offset_minutes: validate_offset(offset)?,
            prompt: validate_prompt(str_arg(args, "prompt")?)?,
            harness,
        };
        let enabled = args.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        let workflow = self.agent_workflow(caller, args).await?;
        let record = self
            .insert_automation(NewAutomation {
                sandbox: caller.sandbox.clone(),
                fields,
                workflow_id: workflow,
                enabled,
                created_by: caller.user.clone(),
                created_via: CreatedVia::Agent,
                actor: caller.actor.clone(),
                detail: caller.detail.clone(),
            })
            .await?;
        let view = self.automation_view(&record).await?;
        Ok(
            json!({"automation": view, "note": "The automation is visible on the Automations page of this project."}),
        )
    }

    /// `automations_list` agent tool: the project's automations and the ready-made ones.
    ///
    /// # Errors
    /// Store failures.
    pub async fn tool_automations_list(&self, caller: &ProjectCaller) -> AppResult<Value> {
        let records = self.deps.store.automations(&caller.sandbox).await?;
        let mut out = Vec::new();
        for record in &records {
            out.push(self.automation_view(record).await?);
        }
        // The catalog without its prompts, which are long (SPEC-066 AUT-012).
        let ready_made: Vec<Value> = templates::templates()
            .iter()
            .map(|t| {
                json!({
                    "id": t.id,
                    "name": t.name,
                    "description": t.description,
                    "cron": t.cron,
                    "utc_offset_minutes": t.utc_offset_minutes,
                    "workflow_id": t.workflow,
                    "automation_id": records
                        .iter()
                        .find(|a| templates::same_name(t, &a.name))
                        .map(|a| a.id.clone()),
                })
            })
            .collect();
        Ok(json!({"automations": out, "ready_made": ready_made}))
    }

    /// `automation_update` agent tool: enable/disable, schedule, offset, prompt or name.
    ///
    /// # Errors
    /// `NotFound` outside the agent's sandbox, validation errors.
    pub async fn tool_automation_update(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        self.manage_authority(caller).await?;
        let current = self.agent_automation(caller, args).await?;
        let mut next = current.clone();
        if let Some(name) = args.get("name").and_then(Value::as_str) {
            next.name = validate_name(name)?;
        }
        if let Some(cron) = args.get("cron").and_then(Value::as_str) {
            next.cron = validate_cron(cron)?;
        }
        if let Some(offset) = args.get("utc_offset_minutes").filter(|v| !v.is_null()) {
            let offset = offset
                .as_i64()
                .ok_or_else(|| AppError::Invalid("utc_offset_minutes must be an integer".into()))?;
            next.utc_offset_minutes = validate_offset(offset)?;
        }
        if let Some(prompt) = args.get("prompt").and_then(Value::as_str) {
            next.prompt = validate_prompt(prompt)?;
        }
        if let Some(enabled) = args.get("enabled").and_then(Value::as_bool) {
            next.enabled = enabled;
        }
        self.save_automation(&current, &next, &caller.actor, caller.detail.clone())
            .await?;
        let stored = self
            .deps
            .store
            .automation(&current.id)
            .await?
            .ok_or(AppError::NotFound)?;
        Ok(json!({"automation": self.automation_view(&stored).await?}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_round_trip() {
        for state in [
            RunState::Scheduled,
            RunState::TaskCreated,
            RunState::SkippedPreviousActive,
            RunState::SkippedDisabled,
            RunState::Failed,
        ] {
            assert_eq!(RunState::parse(state.as_str()), Some(state));
        }
        assert_eq!(RunState::parse("skipped"), None);
        assert_eq!(CreatedVia::parse("agent"), CreatedVia::Agent);
        assert_eq!(CreatedVia::parse("anything"), CreatedVia::Ui);
    }

    #[test]
    fn run_key_uses_milliseconds() {
        assert_eq!(
            run_key("a1", Timestamp(1_700_000_000_000)),
            "a1@1700000000000"
        );
    }

    #[test]
    fn validation_rejects_bad_input() {
        assert!(validate_cron("61 * * * *").is_err());
        assert!(validate_cron("* * *").is_err());
        assert_eq!(
            validate_cron("  0 9 * * MON  ").ok().as_deref(),
            Some("0 9 * * MON")
        );
        assert!(validate_offset(i64::from(MAX_UTC_OFFSET_MINUTES) + 1).is_err());
        assert!(validate_offset(-i64::from(MAX_UTC_OFFSET_MINUTES) - 1).is_err());
        assert_eq!(validate_offset(-300).ok(), Some(-300));
        assert!(validate_offset(i64::MAX).is_err());
        assert!(validate_prompt("   ").is_err());
        assert!(validate_prompt(&"x".repeat(MAX_PROMPT_BYTES + 1)).is_err());
        assert!(validate_name("").is_err());
        assert!(validate_harness("gpt").is_err());
    }

    #[test]
    fn report_counts_by_state() {
        let mut report = SchedulerReport::default();
        let mut run = AutomationRunRecord {
            automation_id: "a".into(),
            scheduled_for: Timestamp(0),
            state: RunState::TaskCreated,
            task_id: None,
            error: None,
            created_at: Timestamp(0),
        };
        report.count(&run);
        run.state = RunState::SkippedDisabled;
        report.count(&run);
        run.state = RunState::Failed;
        report.count(&run);
        assert_eq!(
            report,
            SchedulerReport {
                created: 1,
                skipped: 1,
                failed: 1,
                stale: 0
            }
        );
    }
}
