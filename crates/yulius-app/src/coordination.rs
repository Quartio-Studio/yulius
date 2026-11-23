//! Agent-side coordination: task awareness, scopes and steps (SPEC-001 COM-001, COM-003,
//! STP-001/002). Callers are agents authenticated by their run's task token.

use serde_json::{Value, json};
use yulius_domain::ids::{SandboxId, StepId, TaskId, UserId};
use yulius_domain::task::{AttentionFacts, TaskStatus};

use crate::auth::rfc3339;
use crate::context::App;
use crate::discipline::{step_by_title, step_states};
use crate::error::{AppError, AppResult};
use crate::model::{AttemptRecord, StepRecord, TaskRecord};
use yulius_domain::steps::{self as rules, By, PlannedStep, StepStatus};

/// Step statuses.
pub const STEP_STATUSES: [&str; 5] = ["pending", "in_progress", "done", "skipped", "blocked"];
/// Maximum steps per task (a plan of similar-effort steps, STP-006).
pub const MAX_STEPS: usize = rules::MAX_STEPS;

/// The agent run behind a task token.
#[derive(Debug, Clone)]
pub struct AgentContext {
    /// The run.
    pub attempt: AttemptRecord,
    /// Its task.
    pub task: TaskRecord,
}

impl AgentContext {
    /// Actor label recorded in histories.
    #[must_use]
    pub fn actor(&self) -> String {
        format!("task:{}", self.task.number)
    }

    /// The task's agent as a caller of the project-management tools: it acts for the task's
    /// owner (SPEC-037 MGT-002, SPEC-070).
    #[must_use]
    pub fn caller(&self) -> ProjectCaller {
        ProjectCaller {
            sandbox: self.task.sandbox_id.clone(),
            user: self.task.owner.clone(),
            task: Some(self.task.clone()),
            channel: Channel::Task,
            actor: self.actor(),
            detail: json!({"task_id": self.task.id.as_str(), "task_number": self.task.number}),
        }
    }
}

/// The channel a project-management tool is called through, named where it matters to the
/// reader (a message typed into a task says who sent it and how; SPEC-322).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// A task's agent (task token).
    Task,
    /// A person's Superagent, their Cockpit (`ck_` token).
    Superagent,
    /// A person's own agent through the remote MCP (`operate` token; SPEC-247).
    Remote,
    /// A person's own linked Telegram chat (SPEC-512 TGI-002).
    Telegram,
}

/// Who calls a project-management tool (tasks, automations, workflows, sessions): a task's
/// agent, a person's Cockpit or their own agent through the remote MCP (SPEC-037, SPEC-247).
#[derive(Debug, Clone)]
pub struct ProjectCaller {
    /// The project's sandbox.
    pub sandbox: SandboxId,
    /// The person on whose authority it acts.
    pub user: UserId,
    /// The caller's own task; `None` for a Cockpit or the remote MCP.
    pub task: Option<TaskRecord>,
    /// The channel it calls through.
    pub channel: Channel,
    /// Actor label recorded in histories and audits.
    pub actor: String,
    /// Audit detail naming the caller.
    pub detail: Value,
}

fn str_arg<'a>(args: &'a Value, key: &str) -> AppResult<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::Invalid(format!("missing string argument `{key}`")))
}

impl App {
    /// Resolves a task token to its live run.
    ///
    /// # Errors
    /// `Unauthenticated` for unknown tokens or runs that are no longer live.
    pub async fn agent_context(&self, token: &str) -> AppResult<AgentContext> {
        let hash = self.deps.crypto.sha256_hex(token.as_bytes());
        let attempt = self
            .deps
            .store
            .attempt_by_token(&hash)
            .await?
            .ok_or(AppError::Unauthenticated)?;
        if attempt.state.is_terminal() {
            return Err(AppError::Unauthenticated);
        }
        let task = self
            .deps
            .store
            .task(&attempt.task_id)
            .await?
            .ok_or(AppError::Unauthenticated)?;
        Ok(AgentContext { attempt, task })
    }

    pub(crate) async fn task_brief(&self, task: &TaskRecord) -> AppResult<Value> {
        self.verified_brief(task, false, false).await
    }

    async fn verified_brief(
        &self,
        task: &TaskRecord,
        person: bool,
        detail: bool,
    ) -> AppResult<Value> {
        let requests = tokio::sync::OnceCell::new();
        let parked = if task.status == TaskStatus::Parked {
            Some(
                self.parked_status(task, &requests, self.status_deadline())
                    .await,
            )
        } else {
            None
        };
        self.brief_from_status(task, person, detail, parked, &requests)
            .await
    }

    async fn list_attention_facts(
        &self,
        task: &TaskRecord,
        requests: &tokio::sync::OnceCell<std::collections::HashSet<String>>,
    ) -> AppResult<AttentionFacts> {
        let requested = requests
            .get_or_init(|| self.browser_requests(&task.sandbox_id))
            .await;
        self.task_attention_facts(task, requested).await
    }

    async fn brief_from_status(
        &self,
        task: &TaskRecord,
        person: bool,
        detail: bool,
        parked: Option<crate::sandboxes::ParkedStatus>,
        requests: &tokio::sync::OnceCell<std::collections::HashSet<String>>,
    ) -> AppResult<Value> {
        let (mut facts, snapshot, pr) = match parked {
            Some(status) => (status.facts, status.steps, status.pr),
            None => (self.list_attention_facts(task, requests).await?, None, None),
        };
        let steps = match snapshot {
            Some(steps) => steps,
            None => self.deps.store.steps(&task.id).await?,
        };
        if task.status == TaskStatus::Parked {
            facts.plan_open = crate::sandboxes::plan_open(&steps);
        }
        let mut brief = self
            .task_brief_with_steps(task, facts, person, &steps, pr.map(|pr| pr.number))
            .await?;
        if let Some(pr) = pr {
            brief["parked_pr"] = json!(pr);
        }
        if detail {
            brief["prompt"] = json!(task.prompt.chars().take(4000).collect::<String>());
            brief["steps_list"] = json!(steps.iter().map(|s| json!({"id": s.id.as_str(), "title": s.title, "status": s.status, "note": s.note})).collect::<Vec<_>>());
            let wait_history = self.wait_history_view(task).await;
            if !wait_history.is_empty() {
                brief["wait_history"] = json!(wait_history);
            }
        }
        Ok(brief)
    }

    async fn task_brief_with_steps(
        &self,
        task: &TaskRecord,
        mut facts: AttentionFacts,
        person: bool,
        steps: &[StepRecord],
        pr_hint: Option<u64>,
    ) -> AppResult<Value> {
        let mut brief = self.task_brief_plain(task, steps).await?;
        // Where the task list shows it, and why it waits for a person or is held (SPEC-487
        // QST-001): what its page says, so nobody has to paste it.
        // What it runs on (SPEC-487 SUB-002): its live run's subscription, and the model and
        // effort it was given (none: the instance's defaults).
        brief["subscription"] = json!(self.run_subscription(task).await);
        brief["model"] = json!(task.model);
        brief["effort"] = json!(task.effort);
        if let Some(fields) = brief.as_object_mut() {
            let question = self.waits_why(task, person, &facts, pr_hint).await;
            if task.status == TaskStatus::Parked {
                facts.browser_request |= question.contains_key("needs_you");
                facts.held_prompt |= question.contains_key("held_prompt");
            }
            fields.insert(
                "shows".into(),
                json!(crate::task_questions::group_name(facts.group())),
            );
            fields.extend(question);
        }
        Ok(brief)
    }

    async fn task_brief_plain(&self, task: &TaskRecord, steps: &[StepRecord]) -> AppResult<Value> {
        let p = self.step_progress(&task.id, steps).await?;
        let chain = self.deps.store.chain(&task.id).await?;
        let waits = task.status == yulius_domain::task::TaskStatus::Chained;
        Ok(json!({
            "task_id": task.id.as_str(),
            "number": task.number,
            "title": task.title,
            "harness": task.harness.as_str(),
            "status": task.status.as_str(),
            "activity": task.activity.map(|a| a.as_str()),
            "branch": task.branch,
            "spec_path": task.spec_path,
            "steps": {"done": p.done, "total": p.total, "percent": p.percent, "current": p.current, "first_total": p.first_total},
            // How it waits to start, as stored (SPEC-487 CHN-004): after one task (TSK-012), or
            // on its whole start rule (SPEC-017), never the rule's first task alone.
            "runs_after": chain.as_ref().filter(|c| c.rule.is_none()).map(|c| c.after_number),
            "start_rule": chain.as_ref().filter(|_| waits).and_then(|c| c.rule.clone()),
            // Why it will not start by itself any more, while it waits.
            "start_blocked": chain.as_ref().filter(|_| waits).and_then(|c| c.blocked_reason.clone()),
            "created_at": rfc3339(task.created_at),
            "updated_at": rfc3339(task.updated_at),
        }))
    }

    /// `tasks_list`: previous and parallel tasks of the agent's sandbox (COM-001), without a
    /// personal task's free text for a task that does not hold the same secrets (SPEC-625).
    ///
    /// # Errors
    /// Store failures; a task's agent whose owner may not view the project.
    pub async fn tool_tasks_list(&self, caller: &ProjectCaller, args: &Value) -> AppResult<Value> {
        self.require_reader(caller).await?;
        let scope = args.get("scope").and_then(Value::as_str).unwrap_or("all");
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(30)
            .min(200);
        let mut tasks = self
            .deps
            .store
            .tasks(&caller.sandbox, scope == "archived")
            .await?;
        if scope == "all" {
            tasks.extend(self.deps.store.tasks(&caller.sandbox, true).await?);
        }
        if let Some(own) = &caller.task {
            tasks.retain(|t| t.id != own.id);
        }
        tasks.retain(|t| match scope {
            "active" => Self::is_active_status(t.status),
            "recent" => !t.status.is_live(),
            // Only a task that has not ended waits for a person.
            "needs_you" => !t.status.is_resting(),
            _ => true,
        });
        tasks.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
        let limit = usize::try_from(limit).unwrap_or(30);
        let filtered = scope == "needs_you";
        if !filtered || limit == 0 {
            tasks.truncate(limit);
        }
        let mut rows = self
            .list_briefs(&tasks, limit, filtered, caller.task.is_none())
            .await?;
        // A personal task's free text stays with tasks holding the same secrets (PSV-004).
        for row in &mut rows {
            if let Some(task) = tasks.iter().find(|t| row["task_id"] == t.id.as_str()) {
                self.withhold_from(caller, task, row).await?;
            }
        }
        let mut listing = json!({"tasks": rows});
        if let Some(own) = &caller.task {
            listing["you"] = self.task_brief(own).await?;
        }
        if let Some(wait) = self.github_wait() {
            listing["github_wait"] = json!(wait);
        }
        if self.github_budget().await?.display_paused {
            listing["github_refresh_note"] =
                json!("Live GitHub display refreshes are paused to reserve requests for delivery.");
        }
        Ok(listing)
    }

    /// Apply the owner's limit after the Needs you filter. Only time spent on
    /// parked verification uses its budget. A slow live row cannot spend it.
    async fn list_briefs(
        &self,
        tasks: &[TaskRecord],
        limit: usize,
        filtered: bool,
        person: bool,
    ) -> AppResult<Vec<Value>> {
        let requests = tokio::sync::OnceCell::new();
        let mut rows = Vec::new();
        if !filtered {
            let mut verified = self
                .parked_list_statuses(tasks.iter().map(|task| (task, &requests)).collect())
                .await;
            for task in tasks {
                rows.push(
                    self.brief_from_status(
                        task,
                        person,
                        false,
                        verified.remove(&task.id),
                        &requests,
                    )
                    .await?,
                );
            }
            return Ok(rows);
        }
        let mut remaining = std::time::Duration::from_millis(self.config.parked_pr_read_ms);
        let mut parks_left = tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Parked)
            .count();
        let mut pending = tasks;
        while !pending.is_empty() && rows.len() < limit {
            let count = pending
                .len()
                .min(crate::sandboxes::PARKED_READS)
                .min(limit - rows.len());
            let (batch, rest) = pending.split_at(count);
            pending = rest;
            let parked: Vec<_> = batch
                .iter()
                .filter(|t| t.status == TaskStatus::Parked)
                .map(|task| (task, &requests))
                .collect();
            let mut verified = std::collections::HashMap::new();
            if !parked.is_empty() {
                let start = tokio::time::Instant::now();
                let share = remaining / u32::try_from(parks_left).unwrap_or(u32::MAX)
                    * u32::try_from(parked.len()).unwrap_or(u32::MAX);
                parks_left -= parked.len();
                let deadline = start + share;
                verified.extend(self.read_parked_batch(&parked, deadline).await);
                remaining = remaining.saturating_sub(start.elapsed());
            }
            for task in batch {
                if task.status != TaskStatus::Parked {
                    let facts = self.list_attention_facts(task, &requests).await?;
                    if facts.needs_you() {
                        let steps = self.deps.store.steps(&task.id).await?;
                        rows.push(
                            self.task_brief_with_steps(task, facts, person, &steps, None)
                                .await?,
                        );
                    }
                    continue;
                }
                // Parked verification can finish before a new owner decision is recorded.
                let row = self
                    .brief_from_status(task, person, false, verified.remove(&task.id), &requests)
                    .await?;
                if row["shows"] == "needs_you" {
                    rows.push(row);
                }
            }
        }
        Ok(rows)
    }

    /// `task_get`: one task's details including steps and recent messages it sent, and its
    /// owner, delivery, claims, waits and screen (SPEC-625); for a task's agent, read-only and
    /// without a personal task's free text (PSV-004).
    ///
    /// # Errors
    /// `NotFound` outside the agent's sandbox; a task's agent whose owner may not view it.
    pub async fn tool_task_get(&self, caller: &ProjectCaller, args: &Value) -> AppResult<Value> {
        self.require_reader(caller).await?;
        let task = self.agent_task_ref(caller, args).await?;
        let mut brief = self
            .verified_brief(&task, caller.task.is_none(), true)
            .await?;
        // Its owner, delivery, claims, waits and last words (SPEC-625 PSV-001); a personal task's
        // free text stays with tasks holding the same secrets (PSV-004).
        let withheld = self.withholds_from(caller, &task).await?;
        self.add_peer_status(&task, &mut brief, caller.task.is_none(), withheld)
            .await?;
        if withheld {
            crate::peer_status::withhold(
                &mut brief,
                task.number,
                &self.login_of(&task.owner).await,
            );
        }
        Ok(brief)
    }

    pub(crate) async fn agent_task_ref(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<TaskRecord> {
        if let Some(number) = args.get("number").and_then(Value::as_u64) {
            let number = u32::try_from(number).map_err(|_| AppError::NotFound)?;
            return self
                .deps
                .store
                .task_by_number(&caller.sandbox, number)
                .await?
                .ok_or(AppError::NotFound);
        }
        let id = args
            .get("task_id")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Invalid("name the task by `task_id` or `number`".into()))?;
        let id = TaskId::parse(id).map_err(|_| AppError::NotFound)?;
        self.deps
            .store
            .task(&id)
            .await?
            .filter(|t| t.sandbox_id == caller.sandbox)
            .ok_or(AppError::NotFound)
    }

    /// `project_context` (COM-004, DSC-009): the session briefing rebuilt now — project summary,
    /// architecture and decision index, recent changes, delivery rules and parallel tasks.
    ///
    /// # Errors
    /// Store failures.
    pub async fn tool_project_context(&self, ctx: &AgentContext) -> AppResult<Value> {
        let task = self
            .deps
            .store
            .task(&ctx.task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        let context = self.briefing(&task, ctx.attempt.harness).await?;
        Ok(json!({ "context": context }))
    }

    /// `steps_set`: replaces the task's plan (STP-001, STP-006): 1 to [`MAX_STEPS`] unique steps,
    /// each with an optional status and effort (1-3). A step whose title matches a current step
    /// keeps its identity, note, status and effort unless given; a finished step stays finished.
    ///
    /// # Errors
    /// `Invalid` input, with the rule that was broken.
    pub async fn tool_steps_set(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let items = args
            .get("steps")
            .and_then(Value::as_array)
            .ok_or_else(|| AppError::Invalid("missing array `steps`".into()))?;
        let mut plan = Vec::with_capacity(items.len().min(MAX_STEPS + 1));
        for item in items.iter().take(MAX_STEPS + 1) {
            let (title, status, effort) = match item {
                Value::String(t) => (t.clone(), None, None),
                Value::Object(o) => (
                    o.get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    o.get("status").and_then(Value::as_str),
                    o.get("effort"),
                ),
                _ => (String::new(), None, None),
            };
            let status = status
                .map(|s| {
                    StepStatus::parse(s).ok_or_else(|| {
                        AppError::Invalid(format!("status must be one of {STEP_STATUSES:?}"))
                    })
                })
                .transpose()?;
            let effort = effort
                .map(|e| {
                    e.as_u64()
                        .and_then(|n| u8::try_from(n).ok())
                        .ok_or_else(|| AppError::Invalid("effort must be 1, 2 or 3".into()))
                })
                .transpose()?;
            plan.push(PlannedStep {
                title,
                effort,
                status,
            });
        }
        if items.len() > MAX_STEPS {
            return Err(AppError::Invalid(
                rules::StepError::TooManySteps { count: items.len() }.to_string(),
            ));
        }
        let runtime = self.runtime(&ctx.task.id);
        let steps_lock = runtime.steps_lock.lock().await;
        let (existing, states) = self.step_states_of(&ctx.task.id).await?;
        let resolved = rules::validate_plan(&plan, &states, By::Agent)
            .map_err(|err| AppError::Invalid(err.to_string()))?;
        let now = self.now();
        let mut steps = Vec::with_capacity(resolved.len());
        for (i, step) in resolved.into_iter().enumerate() {
            let previous = step.previous.and_then(|p| existing.get(p));
            steps.push(StepRecord {
                id: previous
                    .map_or_else(
                        || StepId::parse(&self.deps.crypto.new_id()),
                        |p| Ok(p.id.clone()),
                    )
                    .map_err(AppError::internal)?,
                task_id: ctx.task.id.clone(),
                ordinal: u32::try_from(i + 1).unwrap_or(u32::MAX),
                status: step.status.as_str().to_owned(),
                title: step.title,
                note: previous.and_then(|p| p.note.clone()),
                updated_by: Some(ctx.actor()),
                updated_at: Some(now),
                effort: step.effort,
            });
        }
        self.deps
            .store
            .replace_steps(&ctx.task.id, &steps, &ctx.actor(), now)
            .await?;
        drop(steps_lock);
        self.publish_steps(&ctx.task).await;
        self.tool_steps_get(ctx).await
    }

    /// `step_update`: changes one step's status/note (STP-007): at most
    /// [`rules::MAX_IN_PROGRESS`] steps in progress, a finished step stays finished, blocked and
    /// skipped need a note.
    ///
    /// # Errors
    /// `Invalid` (with the rule that was broken) or `NotFound`.
    pub async fn tool_step_update(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let status = str_arg(args, "status")?;
        let parsed = StepStatus::parse(status)
            .ok_or_else(|| AppError::Invalid(format!("status must be one of {STEP_STATUSES:?}")))?;
        let runtime = self.runtime(&ctx.task.id);
        let steps_lock = runtime.steps_lock.lock().await;
        let (steps, states) = self.step_states_of(&ctx.task.id).await?;
        let step = if let Some(id) = args.get("step_id").and_then(Value::as_str) {
            steps.iter().find(|s| s.id.as_str() == id)
        } else if let Some(n) = args.get("ordinal").and_then(Value::as_u64) {
            steps.iter().find(|s| u64::from(s.ordinal) == n)
        } else {
            step_by_title(&steps, str_arg(args, "title")?)
        }
        .ok_or(AppError::NotFound)?;
        let index = steps
            .iter()
            .position(|s| s.id == step.id)
            .ok_or(AppError::NotFound)?;
        let note = args.get("note").and_then(Value::as_str);
        rules::check_update(&states, index, parsed, note, By::Agent)
            .map_err(|err| AppError::Invalid(err.to_string()))?;
        self.deps
            .store
            .update_step(
                &step.id,
                status,
                note.map(str::trim),
                &ctx.actor(),
                self.now(),
            )
            .await?;
        drop(steps_lock);
        self.publish_steps(&ctx.task).await;
        let mut record = self.tool_steps_get(ctx).await?;
        // Which step changed, so the answer can say it (SPEC-086 TOL-002).
        record["updated"] = json!({"ordinal": step.ordinal, "title": step.title, "status": status});
        Ok(record)
    }

    /// `steps_get`: the task's steps, the effort-weighted progress and the current nudges.
    ///
    /// # Errors
    /// Store failures.
    pub async fn tool_steps_get(&self, ctx: &AgentContext) -> AppResult<Value> {
        let records = self.deps.store.steps(&ctx.task.id).await?;
        let timings = self.deps.store.step_timings(&ctx.task.id).await?;
        let states = step_states(&records, &timings);
        let p = self.step_progress(&ctx.task.id, &records).await?;
        let nudges = self.step_nudges(&ctx.task, &states).await;
        Ok(json!({
            "progress": {"done": p.done, "total": p.total, "percent": p.percent, "current": p.current, "first_total": p.first_total},
            "steps": records.iter().zip(&states).map(|(s, st)| json!({
                "id": s.id.as_str(),
                "ordinal": s.ordinal,
                "title": s.title,
                "status": s.status,
                "effort": s.effort,
                "note": s.note,
                "started_at": st.started_at.map(rfc3339),
                "finished_at": st.finished_at.map(rfc3339),
            })).collect::<Vec<_>>(),
            "nudges": nudges,
        }))
    }

    pub(crate) async fn publish_steps(&self, task: &TaskRecord) {
        if crate::hooks::defer_task_update(&task.id, true) {
            return;
        }
        self.events.publish(
            Some(task.sandbox_id.clone()),
            false,
            yulius_contracts::events::UiEvent::StepsUpdated {
                task_id: task.id.to_string(),
            },
        );
        self.publish_task(&task.id).await;
    }
}

#[cfg(test)]
mod tests {
    use crate::context::App;
    use yulius_domain::task::{FailReason, StopReason, TaskStatus as S};

    fn is_active(status: S) -> bool {
        App::is_active_status(status)
    }

    #[test]
    fn active_lists_tasks_at_work_or_waiting_to_work() {
        for status in [
            S::Queued,
            S::Chained,
            S::Prepared,
            S::Starting,
            S::Running,
            S::Switching,
            S::WaitingCapacity,
            S::Stopping,
        ] {
            assert!(is_active(status), "{status:?}");
        }
        for status in [
            S::Closed,
            S::Stopped(StopReason::Crashed),
            S::Failed(FailReason::SpawnError),
        ] {
            assert!(!is_active(status), "{status:?}");
        }
    }
}
