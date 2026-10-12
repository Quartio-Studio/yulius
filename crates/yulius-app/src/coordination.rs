//! Agent-side coordination: task awareness, messages and steps (SPEC-001 COM-001..003,
//! STP-001/002). Callers are agents authenticated by their run's task token.

use serde_json::{Value, json};
use yulius_domain::ids::{MessageId, StepId, TaskId};

use crate::auth::rfc3339;
use crate::context::App;
use crate::discipline::{step_by_title, step_states};
use crate::error::{AppError, AppResult};
use crate::model::{AttemptRecord, MessageRecord, StepRecord, TaskRecord};
use crate::tasks::progress;
use yulius_domain::steps::{self as rules, By, PlannedStep, StepStatus};

/// Message kinds agents may send.
pub const MESSAGE_KINDS: [&str; 3] = ["info", "question", "request"];
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

    async fn task_brief(&self, task: &TaskRecord) -> AppResult<Value> {
        let steps = self.deps.store.steps(&task.id).await?;
        let p = progress(&steps);
        Ok(json!({
            "task_id": task.id.as_str(),
            "number": task.number,
            "title": task.title,
            "harness": task.harness.as_str(),
            "status": task.status.as_str(),
            "activity": task.activity.map(|a| a.as_str()),
            "branch": task.branch,
            "spec_path": task.spec_path,
            "steps": {"done": p.done, "total": p.total, "percent": p.percent, "current": p.current},
            // A chained task runs after this task number (TSK-012).
            "runs_after": self.deps.store.chain(&task.id).await?.map(|c| c.after_number),
            "created_at": rfc3339(task.created_at),
            "updated_at": rfc3339(task.updated_at),
        }))
    }

    /// `tasks_list`: previous and parallel tasks of the agent's sandbox (COM-001).
    ///
    /// # Errors
    /// Store failures.
    pub async fn tool_tasks_list(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let scope = args.get("scope").and_then(Value::as_str).unwrap_or("all");
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(30)
            .min(200);
        let mut tasks = self.deps.store.tasks(&ctx.task.sandbox_id, false).await?;
        if scope == "all" || scope == "archived" {
            tasks.extend(self.deps.store.tasks(&ctx.task.sandbox_id, true).await?);
        }
        tasks.retain(|t| t.id != ctx.task.id);
        tasks.retain(|t| match scope {
            "active" => {
                t.status.is_live()
                    || matches!(
                        t.status,
                        yulius_domain::task::TaskStatus::Queued
                            | yulius_domain::task::TaskStatus::Chained
                    )
            }
            "recent" => !t.status.is_live(),
            _ => true,
        });
        tasks.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
        let mut out = Vec::new();
        for t in tasks.iter().take(usize::try_from(limit).unwrap_or(30)) {
            out.push(self.task_brief(t).await?);
        }
        Ok(json!({"you": self.task_brief(&ctx.task).await?, "tasks": out}))
    }

    /// `task_get`: one task's details including steps and recent messages it sent.
    ///
    /// # Errors
    /// `NotFound` outside the agent's sandbox.
    pub async fn tool_task_get(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let task = self.agent_task_ref(ctx, args).await?;
        let steps = self.deps.store.steps(&task.id).await?;
        let mut brief = self.task_brief(&task).await?;
        brief["prompt"] = json!(task.prompt.chars().take(4000).collect::<String>());
        brief["steps_list"] = json!(steps.iter().map(|s| json!({"id": s.id.as_str(), "title": s.title, "status": s.status, "note": s.note})).collect::<Vec<_>>());
        Ok(brief)
    }

    async fn agent_task_ref(&self, ctx: &AgentContext, args: &Value) -> AppResult<TaskRecord> {
        if let Some(number) = args.get("number").and_then(Value::as_u64) {
            let number = u32::try_from(number).map_err(|_| AppError::NotFound)?;
            return self
                .deps
                .store
                .task_by_number(&ctx.task.sandbox_id, number)
                .await?
                .ok_or(AppError::NotFound);
        }
        let id = TaskId::parse(str_arg(args, "task_id")?).map_err(|_| AppError::NotFound)?;
        self.deps
            .store
            .task(&id)
            .await?
            .filter(|t| t.sandbox_id == ctx.task.sandbox_id)
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

    /// `message_send`: the agent's message to the people using Yulius (`to="people"`, COM-005,
    /// #46): shown in this task's Messages and in their notifications. Messages to other tasks
    /// are gone (COM-002 withdrawn, MSG-001); notices come from Yulius itself (`tell_task`).
    ///
    /// # Errors
    /// `Invalid` input.
    pub async fn tool_message_send(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        if args.get("to").and_then(Value::as_str) != Some("people") {
            return Err(AppError::Invalid(
                "message_send only tells or asks the people using Yulius: pass to=\"people\" (messages to other tasks are not supported)".into(),
            ));
        }
        let body = str_arg(args, "body")?.trim();
        if body.is_empty() || body.len() > 16 * 1024 {
            return Err(AppError::Invalid("body must be 1 byte to 16 KiB".into()));
        }
        let kind = args.get("kind").and_then(Value::as_str).unwrap_or("info");
        if !MESSAGE_KINDS.contains(&kind) {
            return Err(AppError::Invalid(format!(
                "kind must be one of {MESSAGE_KINDS:?}"
            )));
        }
        let subject: String = args
            .get("subject")
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(200)
            .collect();
        let record = MessageRecord {
            id: MessageId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?,
            sandbox_id: ctx.task.sandbox_id.clone(),
            from_task: Some(ctx.task.id.clone()),
            from_user: None,
            to_task: None,
            kind: kind.to_owned(),
            subject,
            body: body.to_owned(),
            created_at: self.now(),
        };
        self.deps.store.insert_message(&record, &[]).await?;
        // No task receives it: the people see it in this task's thread and notifications.
        let entry = crate::model::InboxEntry {
            message: record.clone(),
            delivered: false,
            read: false,
        };
        if let Ok(view) = self.message_view(&entry).await {
            self.events.publish(
                Some(ctx.task.sandbox_id.clone()),
                false,
                yulius_contracts::events::UiEvent::Message {
                    task_id: ctx.task.id.to_string(),
                    message: view,
                },
            );
        }
        Ok(
            json!({"sent": true, "message_id": record.id.as_str(), "to": "people", "shown": "in this task's Messages and in people's notifications"}),
        )
    }

    /// A notice from Yulius itself to one task (a `request`: it reaches the agent at its next
    /// turn end, and an agent waiting at its prompt is woken for it). Used to keep delivery
    /// going without a person (F 2026-10-11: "the agent is autonomous").
    pub async fn tell_task(&self, task: &crate::model::TaskRecord, subject: &str, body: String) {
        let Ok(id) = MessageId::parse(&self.deps.crypto.new_id()) else {
            return;
        };
        let message = MessageRecord {
            id,
            sandbox_id: task.sandbox_id.clone(),
            from_task: None,
            from_user: None,
            to_task: Some(task.id.clone()),
            kind: "request".into(),
            subject: subject.to_owned(),
            body,
            created_at: self.now(),
        };
        match self
            .deps
            .store
            .insert_message(&message, std::slice::from_ref(&task.id))
            .await
        {
            Ok(()) => self.notify_message(&task.id).await,
            Err(err) => tracing::warn!(error = %err, task = %task.id, "could not tell the task"),
        }
    }

    pub(crate) async fn notify_message(&self, task: &TaskId) {
        if let Some(waker) = self.message_waker.get() {
            waker.message_arrived(task).await;
        }
    }

    /// `messages_inbox`: messages to the agent; marks them read.
    ///
    /// # Errors
    /// Store failures.
    pub async fn tool_messages_inbox(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let unread_only = args
            .get("unread_only")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let entries = self.deps.store.inbox(&ctx.task.id, unread_only, 50).await?;
        let mut out = Vec::new();
        for e in &entries {
            let v = self.message_view(e).await?;
            out.push(json!({"id": v.id, "from": v.from.title, "from_task_id": v.from.task_id, "kind": v.kind, "subject": v.subject, "body": v.body, "at": v.created_at}));
        }
        self.deps.store.mark_read(&ctx.task.id, self.now()).await?;
        self.publish_task(&ctx.task.id).await;
        Ok(json!({"messages": out}))
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
        self.publish_steps(&ctx.task).await;
        self.tool_steps_get(ctx).await
    }

    /// `steps_get`: the task's steps, the effort-weighted progress and the current nudges.
    ///
    /// # Errors
    /// Store failures.
    pub async fn tool_steps_get(&self, ctx: &AgentContext) -> AppResult<Value> {
        let records = self.deps.store.steps(&ctx.task.id).await?;
        let timings = self.deps.store.step_timings(&ctx.task.id).await?;
        let states = step_states(&records, &timings);
        let p = progress(&records);
        let nudges = self.step_nudges(&ctx.task, &states).await;
        Ok(json!({
            "progress": {"done": p.done, "total": p.total, "percent": p.percent, "current": p.current},
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

/// Supervisor callback used to wake idle tasks when messages arrive (SPEC-001 COM-005).
#[async_trait::async_trait]
pub trait MessageWaker: Send + Sync {
    /// A message was stored for `task`.
    async fn message_arrived(&self, task: &TaskId);
}
