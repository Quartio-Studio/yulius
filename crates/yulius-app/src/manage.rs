//! Managing the project from an agent or a Cockpit (SPEC-037, SPEC-066): creating, stopping,
//! archiving and deleting tasks, deleting and running automations and enabling ready-made ones
//! through the Yulius MCP server.
//!
//! Contract:
//! - `task_create` creates a task the way New task does ([`App::create_task_as`]), with origin
//!   `agent`, on the authority of the person the caller acts for (MGT-001, MGT-002);
//! - tools create at most [`MAX_TOOL_TASKS_PER_HOUR`] tasks per project in a rolling hour, task
//!   agents and Cockpits together; the window lives in memory (MGT-003);
//! - `task_stop`, `task_archive`, `task_delete`, `automation_delete` and `automation_run_now` run
//!   the UI's function for that person, never on the caller's own task, and are audited with the
//!   caller (MGT-008..011); `automation_template_enable` enables a ready-made automation as the
//!   Automations page does (AUT-012).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use yulius_contracts::api::tasks::CreateTask;
use yulius_domain::auto_choice::AUTO;
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;

use crate::auth::AuthContext;
use crate::automations::{CreatedVia, templates};
use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::error::{AppError, AppResult};
use crate::model::{TaskOrigin, TaskRecord, UserRecord};

/// Most tasks tools may create per project in a rolling hour (MGT-003).
pub const MAX_TOOL_TASKS_PER_HOUR: usize = 20;
const HOUR_MS: i64 = 60 * 60 * 1000;

/// Creation times (ms) of tool-created tasks per project, within the last hour.
#[derive(Debug, Default)]
pub(crate) struct ToolTaskWindow(Mutex<HashMap<SandboxId, VecDeque<i64>>>);

impl ToolTaskWindow {
    /// Takes a slot at `now`, or says how many minutes until the next one frees.
    fn reserve(&self, sandbox: &SandboxId, now: i64) -> Result<(), i64> {
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.retain(|_, times| {
            while times.front().is_some_and(|t| now - t >= HOUR_MS) {
                times.pop_front();
            }
            !times.is_empty()
        });
        let times = map.entry(sandbox.clone()).or_default();
        if times.len() >= MAX_TOOL_TASKS_PER_HOUR {
            let oldest = times.front().copied().unwrap_or(now);
            let wait_ms = (oldest + HOUR_MS - now).max(0);
            return Err((wait_ms + 59_999) / 60_000);
        }
        times.push_back(now);
        Ok(())
    }

    /// Gives back a slot taken at `at` by a creation that failed.
    fn release(&self, sandbox: &SandboxId, at: i64) {
        let mut map = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(times) = map.get_mut(sandbox)
            && let Some(index) = times.iter().rposition(|t| *t == at)
        {
            times.remove(index);
        }
    }
}

fn opt_str(args: &Value, key: &str) -> AppResult<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.trim().to_owned()).filter(|t| !t.is_empty())),
        Some(_) => Err(AppError::Invalid(format!("`{key}` must be a string"))),
    }
}

impl App {
    /// The person a caller acts for, when they may still change things in its project: enabled
    /// and with write access, checked at every creating or changing tool call (MGT-002, MGT-005).
    ///
    /// # Errors
    /// `Forbidden` for a disabled person, the access errors of [`App::require`].
    pub(crate) async fn manage_authority(&self, caller: &ProjectCaller) -> AppResult<UserRecord> {
        let user = self
            .deps
            .store
            .user(&caller.user)
            .await?
            .filter(|u| u.disabled_at.is_none())
            .ok_or_else(|| {
                AppError::Forbidden("the person this session acts for is disabled".into())
            })?;
        self.require(&user, &caller.sandbox, Action::Work).await?;
        Ok(user)
    }

    /// The project's task an `after_task` argument names by its number (`task_create`,
    /// `automation_create`, `automation_update`).
    ///
    /// # Errors
    /// `Invalid` for a value that is not a task number or names no task of the project.
    pub(crate) async fn after_task_arg(
        &self,
        caller: &ProjectCaller,
        number: &Value,
    ) -> AppResult<TaskRecord> {
        let number = number
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| AppError::Invalid("`after_task` must be a task number".into()))?;
        self.deps
            .store
            .task_by_number(&caller.sandbox, number)
            .await?
            .ok_or_else(|| {
                AppError::Invalid(format!("task #{number} does not exist in this project"))
            })
    }

    /// `task_create` tool (SPEC-037 MGT-001..003): creates and starts a task in the caller's
    /// project for the person it acts for.
    ///
    /// # Errors
    /// `Invalid` input, `Forbidden` without write access, `Capacity` beyond the hourly bound,
    /// and the errors of New task.
    pub async fn tool_task_create(
        self: &Arc<Self>,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        let prompt = args
            .get("prompt")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Invalid("missing string argument `prompt`".into()))?;
        let after_task_id = match args.get("after_task") {
            None | Some(Value::Null) => None,
            Some(number) => Some(
                self.after_task_arg(caller, number)
                    .await?
                    .id
                    .as_str()
                    .to_owned(),
            ),
        };
        let workflow_id = match opt_str(args, "workflow")? {
            Some(auto) if auto == AUTO => Some(auto),
            Some(_) => self.agent_workflow(caller, args).await?,
            None => None,
        };
        let request = CreateTask {
            prompt: prompt.to_owned(),
            harness: "claude".into(),
            workflow_id,
            model: opt_str(args, "model")?,
            effort: opt_str(args, "effort")?,
            title: opt_str(args, "title")?,
            mode: opt_str(args, "mode")?,
            after_task_id,
            start_rule: opt_str(args, "start_rule")?,
            supertask: match args.get("supertask") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(flag)) => *flag,
                Some(_) => {
                    return Err(AppError::Invalid(
                        "`supertask` must be true or false".into(),
                    ));
                }
            },
        };
        let user = self.manage_authority(caller).await?;
        let now = self.now().0;
        self.tool_tasks
            .reserve(&caller.sandbox, now)
            .map_err(|minutes| {
                AppError::Capacity(format!(
                    "tools may create at most {MAX_TOOL_TASKS_PER_HOUR} tasks an hour in a project; \
                     the next one is possible in {minutes} min"
                ))
            })?;
        let task = match self
            .create_task_as(&user, &caller.sandbox, &request, TaskOrigin::Agent)
            .await
        {
            Ok(task) => task,
            Err(err) => {
                self.tool_tasks.release(&caller.sandbox, now);
                return Err(err);
            }
        };
        tracing::info!(task = %task.id, number = task.number, actor = %caller.actor, "task created by a tool");
        Ok(json!({
            "task": self.task_brief(&task).await?,
            "note": "The task is on the Tasks page of this project.",
        }))
    }
}

/// What a task tool does to another task (MGT-008).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskChange {
    /// `task_stop`.
    Stop,
    /// `task_archive`: archive (`true`) or restore (`false`).
    Archive(bool),
    /// `task_delete`.
    Delete,
}

impl TaskChange {
    fn verb(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Archive(true) => "archive",
            Self::Archive(false) => "restore",
            Self::Delete => "delete",
        }
    }
}

/// The audit detail of a tool's call: the caller's detail with `extra` fields.
fn tool_detail(caller: &ProjectCaller, extra: &Value) -> String {
    let mut detail = caller.detail.clone();
    if let (Some(detail), Some(extra)) = (detail.as_object_mut(), extra.as_object()) {
        detail.extend(extra.clone());
    }
    detail.to_string()
}

impl App {
    /// The person a caller acts for, as the UI's functions take them, once they may change
    /// things in its project ([`App::manage_authority`]).
    async fn manage_context(&self, caller: &ProjectCaller) -> AppResult<AuthContext> {
        Ok(AuthContext {
            user: self.manage_authority(caller).await?,
            csrf_token: None,
            session_hash: None,
        })
    }

    /// `task_stop`, `task_archive` and `task_delete` (SPEC-066 MGT-008..010): the UI's stop,
    /// archive and delete of a task of the caller's project, for the person it acts for, never
    /// on the caller's own task; audited with the caller.
    ///
    /// # Errors
    /// `NotFound` outside the project, `Forbidden` on the caller's own task or without write
    /// access, and the UI's errors (`Conflict` for a live task that is archived or deleted).
    pub async fn tool_task_change(
        self: &Arc<Self>,
        caller: &ProjectCaller,
        args: &Value,
        change: TaskChange,
    ) -> AppResult<Value> {
        let ctx = self.manage_context(caller).await?;
        let task: TaskRecord = self.agent_task_ref(caller, args).await?;
        if caller.task.as_ref().is_some_and(|own| own.id == task.id) {
            return Err(AppError::Forbidden(format!(
                "a task cannot {} itself; Yulius closes and archives a finished task by itself",
                change.verb()
            )));
        }
        let id = task.id.as_str();
        let named = json!({"task": format!("#{} {}", task.number, task.title)});
        let action = match change {
            TaskChange::Delete => {
                self.delete_task_as(&ctx, id, &caller.actor, Some(&caller.detail))
                    .await?;
                tracing::info!(task = %task.id, actor = %caller.actor, "task deleted by a tool");
                return Ok(json!({"deleted": {"task_id": id, "number": task.number}}));
            }
            TaskChange::Stop => {
                self.stop_task(&ctx, id).await?;
                "task.stopped"
            }
            TaskChange::Archive(archived) => {
                self.archive_task(&ctx, id, archived).await?;
                if archived {
                    "task.archived"
                } else {
                    "task.unarchived"
                }
            }
        };
        // The change happened: a failed audit write is logged, not reported as a failed call
        // (which would invite a retry), as for a deletion.
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &caller.actor,
                action,
                Some(id),
                Some(&tool_detail(caller, &named)),
            )
            .await
        {
            tracing::warn!(task = %task.id, error = %err, action, "tool audit failed");
        }
        tracing::info!(task = %task.id, actor = %caller.actor, action, "task changed by a tool");
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        Ok(json!({"task": self.task_brief(&task).await?}))
    }

    /// `automation_delete` (SPEC-066 MGT-011): the UI's delete of an automation of the caller's
    /// project, audited with the caller. Its tasks stay.
    ///
    /// # Errors
    /// `NotFound` outside the project, `Forbidden` without write access.
    pub async fn tool_automation_delete(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        let ctx = self.manage_context(caller).await?;
        let record = self.agent_automation(caller, args).await?;
        self.delete_automation_as(&ctx, &record.id, &caller.actor, Some(&caller.detail))
            .await?;
        Ok(json!({"deleted": {"automation_id": record.id, "name": record.name}}))
    }

    /// `automation_run_now` (SPEC-066 MGT-011): one run now, as Run now on the Automations page
    /// (also when disabled; skipped while its previous task is active), audited with the caller.
    ///
    /// # Errors
    /// `NotFound` outside the project, `Forbidden` without write access, store failures.
    pub async fn tool_automation_run_now(
        self: &Arc<Self>,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        let ctx = self.manage_context(caller).await?;
        let record = self.agent_automation(caller, args).await?;
        let run = self
            .run_automation_now_as(&ctx, &record.id, None, &caller.actor, Some(&caller.detail))
            .await?;
        Ok(json!({"run": run}))
    }

    /// `automation_template_enable` (SPEC-066 AUT-012): enables a ready-made automation by its
    /// catalog id as the Automations page does: created enabled for the person the caller acts
    /// for (`created_via = agent`), or the project's automation of that name enabled unchanged.
    ///
    /// # Errors
    /// `Invalid` for an unknown id (naming the known ones), `Forbidden` without write access,
    /// capacity errors.
    pub async fn tool_automation_template_enable(
        &self,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        let wanted = opt_str(args, "template")?
            .ok_or_else(|| AppError::Invalid("missing string argument `template`".into()))?;
        if templates::template(&wanted).is_none() {
            let known: Vec<&str> = templates::templates()
                .iter()
                .map(|t| t.id.as_str())
                .collect();
            return Err(AppError::Invalid(format!(
                "unknown ready-made automation `{wanted}`; the known ones are: {} (automations_list shows them under ready_made)",
                known.join(", ")
            )));
        }
        let user = self.manage_authority(caller).await?;
        let (automation, created) = self
            .enable_template_as(
                &caller.sandbox,
                &wanted,
                &user.id,
                CreatedVia::Agent,
                &caller.actor,
                caller.detail.clone(),
            )
            .await?;
        Ok(json!({
            "automation": automation,
            "created": created,
            "note": if created {
                "The ready-made automation was added, enabled, to the Automations page of this project."
            } else {
                "The project already had this automation; it is enabled, its content unchanged."
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_holds_twenty_an_hour_and_frees_slots_as_they_age() {
        let window = ToolTaskWindow::default();
        let sandbox = SandboxId::parse("s1").unwrap_or_else(|_| unreachable!());
        let other = SandboxId::parse("s2").unwrap_or_else(|_| unreachable!());
        for i in 0..MAX_TOOL_TASKS_PER_HOUR {
            let at = i64::try_from(i).unwrap_or_default() * 1000;
            assert_eq!(window.reserve(&sandbox, at), Ok(()));
        }
        assert_eq!(window.reserve(&sandbox, 30_000), Err(60));
        assert_eq!(window.reserve(&other, 30_000), Ok(()), "per project");
        // A failed creation gives its slot back.
        window.release(&sandbox, 19_000);
        assert_eq!(window.reserve(&sandbox, 31_000), Ok(()));
        // The oldest frees one hour after it was taken.
        assert_eq!(window.reserve(&sandbox, HOUR_MS - 1), Err(1));
        assert_eq!(window.reserve(&sandbox, HOUR_MS), Ok(()));
    }
}
