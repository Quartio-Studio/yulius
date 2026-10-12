//! Managing the project from an agent or a Cockpit (SPEC-037): creating tasks through the Yulius
//! MCP server.
//!
//! Contract:
//! - `task_create` creates a task the way New task does ([`App::create_task_as`]), with origin
//!   `agent`, on the authority of the person the caller acts for (MGT-001, MGT-002);
//! - tools create at most [`MAX_TOOL_TASKS_PER_HOUR`] tasks per project in a rolling hour, task
//!   agents and Cockpits together; the window lives in memory (MGT-003).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use yulius_contracts::api::tasks::CreateTask;
use yulius_domain::auto_choice::AUTO;
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;

use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::error::{AppError, AppResult};
use crate::model::{TaskOrigin, UserRecord};

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
            Some(number) => {
                let number = number
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| {
                        AppError::Invalid("`after_task` must be a task number".into())
                    })?;
                let task = self
                    .deps
                    .store
                    .task_by_number(&caller.sandbox, number)
                    .await?
                    .ok_or_else(|| {
                        AppError::Invalid(format!("task #{number} does not exist in this project"))
                    })?;
                Some(task.id.as_str().to_owned())
            }
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
