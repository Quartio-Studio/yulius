//! Agent-side coordination: task awareness, messages and steps (SPEC-001 COM-001..003,
//! STP-001/002). Callers are agents authenticated by their run's task token.

use serde_json::{Value, json};
use yulius_domain::ids::{MessageId, StepId, TaskId};

use crate::auth::rfc3339;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{AttemptRecord, MessageRecord, StepRecord, TaskRecord};
use crate::tasks::progress;

/// Message kinds agents may send.
pub const MESSAGE_KINDS: [&str; 4] = ["info", "question", "request", "scope"];
/// Step statuses.
pub const STEP_STATUSES: [&str; 5] = ["pending", "in_progress", "done", "skipped", "blocked"];
/// Maximum steps per task.
pub const MAX_STEPS: usize = 100;

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
            "steps": {"done": p.done, "total": p.total, "current": p.current},
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
            "active" => t.status.is_live() || t.status == yulius_domain::task::TaskStatus::Queued,
            "recent" => !t.status.is_live(),
            _ => true,
        });
        tasks.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
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

    /// `message_send`: harness-independent message to a task (by id/number) or broadcast (COM-002).
    ///
    /// # Errors
    /// `Invalid` input, `NotFound` recipient.
    pub async fn tool_message_send(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
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
        let broadcast = args.get("to").and_then(Value::as_str) == Some("all");
        let (to_task, recipients) = if broadcast {
            let mut recipients: Vec<TaskId> = self
                .deps
                .store
                .tasks(&ctx.task.sandbox_id, false)
                .await?
                .into_iter()
                .filter(|t| t.id != ctx.task.id)
                .map(|t| t.id)
                .collect();
            recipients.dedup();
            (None, recipients)
        } else {
            let target = self.agent_task_ref(ctx, args).await?;
            if target.id == ctx.task.id {
                return Err(AppError::Invalid("cannot message yourself".into()));
            }
            (Some(target.id.clone()), vec![target.id])
        };
        let record = MessageRecord {
            id: MessageId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?,
            sandbox_id: ctx.task.sandbox_id.clone(),
            from_task: Some(ctx.task.id.clone()),
            from_user: None,
            to_task,
            kind: kind.to_owned(),
            subject,
            body: body.to_owned(),
            created_at: self.now(),
        };
        self.deps.store.insert_message(&record, &recipients).await?;
        for r in &recipients {
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
                        task_id: r.to_string(),
                        message: view,
                    },
                );
            }
            self.publish_task(r).await;
            self.notify_message(r).await;
        }
        Ok(json!({"sent": true, "message_id": record.id.as_str(), "recipients": recipients.len()}))
    }

    /// Hook for the supervisor to wake idle recipients (no-op until a supervisor is attached).
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

    /// `steps_set`: replaces the task's plan (STP-001); statuses of identical titles are kept.
    ///
    /// # Errors
    /// `Invalid` input.
    pub async fn tool_steps_set(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let items = args
            .get("steps")
            .and_then(Value::as_array)
            .ok_or_else(|| AppError::Invalid("missing array `steps`".into()))?;
        if items.is_empty() || items.len() > MAX_STEPS {
            return Err(AppError::Invalid(format!("provide 1 to {MAX_STEPS} steps")));
        }
        let existing = self.deps.store.steps(&ctx.task.id).await?;
        let now = self.now();
        let mut steps = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let (title, status) = match item {
                Value::String(t) => (t.trim().to_owned(), None),
                Value::Object(o) => (
                    o.get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_owned(),
                    o.get("status").and_then(Value::as_str).map(str::to_owned),
                ),
                _ => (String::new(), None),
            };
            if title.is_empty() || title.len() > 300 {
                return Err(AppError::Invalid(
                    "each step needs a title of 1-300 bytes".into(),
                ));
            }
            if let Some(s) = &status
                && !STEP_STATUSES.contains(&s.as_str())
            {
                return Err(AppError::Invalid(format!(
                    "status must be one of {STEP_STATUSES:?}"
                )));
            }
            let previous = existing.iter().find(|s| s.title == title);
            steps.push(StepRecord {
                id: previous
                    .map_or_else(
                        || StepId::parse(&self.deps.crypto.new_id()),
                        |p| Ok(p.id.clone()),
                    )
                    .map_err(AppError::internal)?,
                task_id: ctx.task.id.clone(),
                ordinal: u32::try_from(i + 1).unwrap_or(u32::MAX),
                status: status
                    .or_else(|| previous.map(|p| p.status.clone()))
                    .unwrap_or_else(|| "pending".into()),
                title,
                note: previous.and_then(|p| p.note.clone()),
                updated_by: Some(ctx.actor()),
                updated_at: Some(now),
            });
        }
        self.deps
            .store
            .replace_steps(&ctx.task.id, &steps, &ctx.actor(), now)
            .await?;
        self.publish_steps(&ctx.task).await;
        self.tool_steps_get(ctx).await
    }

    /// `step_update`: changes one step's status/note.
    ///
    /// # Errors
    /// `Invalid`/`NotFound`.
    pub async fn tool_step_update(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let status = str_arg(args, "status")?;
        if !STEP_STATUSES.contains(&status) {
            return Err(AppError::Invalid(format!(
                "status must be one of {STEP_STATUSES:?}"
            )));
        }
        let steps = self.deps.store.steps(&ctx.task.id).await?;
        let step = if let Some(id) = args.get("step_id").and_then(Value::as_str) {
            steps.iter().find(|s| s.id.as_str() == id)
        } else if let Some(n) = args.get("ordinal").and_then(Value::as_u64) {
            steps.iter().find(|s| u64::from(s.ordinal) == n)
        } else {
            let title = str_arg(args, "title")?;
            steps.iter().find(|s| s.title == title)
        }
        .ok_or(AppError::NotFound)?;
        let note = args.get("note").and_then(Value::as_str);
        self.deps
            .store
            .update_step(&step.id, status, note, &ctx.actor(), self.now())
            .await?;
        self.publish_steps(&ctx.task).await;
        self.tool_steps_get(ctx).await
    }

    /// `steps_get`: the task's steps.
    ///
    /// # Errors
    /// Store failures.
    pub async fn tool_steps_get(&self, ctx: &AgentContext) -> AppResult<Value> {
        let steps = self.deps.store.steps(&ctx.task.id).await?;
        let p = progress(&steps);
        Ok(json!({
            "progress": {"done": p.done, "total": p.total, "current": p.current},
            "steps": steps.iter().map(|s| json!({"id": s.id.as_str(), "ordinal": s.ordinal, "title": s.title, "status": s.status, "note": s.note})).collect::<Vec<_>>(),
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
