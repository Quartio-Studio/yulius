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
//!   Automations page does (AUT-012);
//! - `task_update` changes how a waiting task starts as the UI's change does (SPEC-126), so no
//!   task is recreated to chain it (SPEC-396 ATR-006).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use yulius_contracts::api::tasks::{ChangeStart, CreateTask};
use yulius_domain::auto_choice::AUTO;
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::rule::{MAX_ATOMS, Rule};
use yulius_domain::task::{TaskGroup, TaskStatus};

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

fn flag_arg(args: &Value, key: &str) -> AppResult<bool> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(flag)) => Ok(*flag),
        Some(_) => Err(AppError::Invalid(format!("`{key}` must be true or false"))),
    }
}

fn opt_str(args: &Value, key: &str) -> AppResult<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.trim().to_owned()).filter(|t| !t.is_empty())),
        Some(_) => Err(AppError::Invalid(format!("`{key}` must be a string"))),
    }
}

/// What `after_all_tasks` answers when no task is left to wait for (SPEC-487 CHN-005).
const NOTHING_LEFT: &str = "No task of this project is left to wait for: it starts now.";

/// `after_all_tasks` among `args`, which then holds no other start choice (SPEC-487 CHN-005).
fn after_all_tasks_arg(args: &Value) -> AppResult<bool> {
    if !flag_arg(args, "after_all_tasks")? {
        return Ok(false);
    }
    // A blank text is no choice, as `opt_str` reads it.
    let other = [
        "after_task",
        "start_rule",
        "supertask",
        "prepared",
        "start_at",
    ]
    .into_iter()
    .any(|key| match args.get(key) {
        None | Some(Value::Null | Value::Bool(false)) => false,
        Some(Value::String(text)) => !text.trim().is_empty(),
        Some(_) => true,
    });
    if other {
        return Err(AppError::Invalid(
            "`after_all_tasks` stands alone: give no after_task, start_rule, supertask, prepared or start_at with it".into(),
        ));
    }
    Ok(true)
}

impl App {
    /// The start rule `after_all_tasks` means now (SPEC-487 CHN-005): every task of `sandbox`
    /// that still has work coming is done. Left out: a task that ended or is stopping; one the
    /// task list shows under Done (done, or "Answered": an idle session with nothing open,
    /// which never becomes `done` by itself); a prepared task (it waits for a person or a
    /// time); and `of`, the task whose start changes, with the tasks waiting on it, directly or
    /// through others (waiting for them would be waiting for itself). `None` when no task is
    /// left.
    ///
    /// # Errors
    /// `Invalid` for more tasks than a rule names; store failures.
    async fn all_tasks_rule(
        &self,
        sandbox: &SandboxId,
        of: Option<&TaskRecord>,
    ) -> AppResult<Option<String>> {
        let tasks = self.deps.store.tasks(sandbox, false).await?;
        let mut held: Vec<u32> = of.map(|task| task.number).into_iter().collect();
        while !held.is_empty() {
            let mut grew = false;
            for task in &tasks {
                if task.status != TaskStatus::Chained || held.contains(&task.number) {
                    continue;
                }
                let Some(chain) = self.deps.store.chain(&task.id).await? else {
                    continue;
                };
                let names = match chain.rule.as_deref() {
                    Some(rule) => Rule::parse(rule)
                        .map(|rule| rule.numbers())
                        .unwrap_or_default(),
                    None => vec![chain.after_number],
                };
                if names.iter().any(|number| held.contains(number)) {
                    held.push(task.number);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        let requested = self.browser_requests(sandbox).await;
        let mut numbers = Vec::new();
        for task in &tasks {
            if task.status == TaskStatus::Prepared || held.contains(&task.number) {
                continue;
            }
            // By the list's own rule (`AttentionFacts::group`).
            if !matches!(
                self.task_group(task, &requested).await?,
                TaskGroup::Done | TaskGroup::Ended
            ) {
                numbers.push(task.number);
            }
        }
        numbers.sort_unstable();
        if numbers.len() > MAX_ATOMS {
            return Err(AppError::Invalid(format!(
                "{} tasks of this project have not ended, and a rule names at most {MAX_ATOMS}: name the ones that matter in start_rule",
                numbers.len()
            )));
        }
        let rule = numbers
            .iter()
            .map(|number| format!("#{number} done"))
            .collect::<Vec<_>>()
            .join(" and ");
        Ok(Some(rule).filter(|rule| !rule.is_empty()))
    }

    /// The person a caller acts for, when they may still change things in its project: enabled
    /// and with write access, checked at every creating or changing tool call (MGT-002, MGT-005).
    ///
    /// # Errors
    /// `Forbidden` for a disabled person, the access errors of [`App::require`].
    pub(crate) async fn manage_authority(&self, caller: &ProjectCaller) -> AppResult<UserRecord> {
        self.person_authority(caller, Action::Work).await
    }

    /// The person a caller acts for, when they are enabled and `action` is theirs in its
    /// project, as things are at this call (SPEC-487 SAR-002).
    ///
    /// # Errors
    /// `Forbidden` for a disabled person, the access errors of [`App::require`].
    pub(crate) async fn person_authority(
        &self,
        caller: &ProjectCaller,
        action: Action,
    ) -> AppResult<UserRecord> {
        let user = self
            .deps
            .store
            .user(&caller.user)
            .await?
            .filter(|u| u.disabled_at.is_none())
            .ok_or_else(|| {
                AppError::Forbidden("the person this session acts for is disabled".into())
            })?;
        self.require(&user, &caller.sandbox, action).await?;
        Ok(user)
    }

    /// [`App::manage_authority`] for a tool that configures the project (its automations): the
    /// person also needs `Action::Configure`, which a playground's guest never has (SPEC-410
    /// GST-003), as on the remote MCP and over HTTP.
    ///
    /// # Errors
    /// As [`App::manage_authority`]; `Forbidden` for a guest.
    pub(crate) async fn configure_authority(
        &self,
        caller: &ProjectCaller,
    ) -> AppResult<UserRecord> {
        let user = self.manage_authority(caller).await?;
        self.require(&user, &caller.sandbox, Action::Configure)
            .await?;
        Ok(user)
    }

    /// [`App::manage_context`] for a configuring tool ([`App::configure_authority`]).
    pub(crate) async fn configure_context(&self, caller: &ProjectCaller) -> AppResult<AuthContext> {
        Ok(AuthContext {
            user: self.configure_authority(caller).await?,
            csrf_token: None,
            session_hash: None,
            token: None,
        })
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
        self.task_number_arg(caller, number, "after_task").await
    }

    /// The project's task the argument `key` names by its number.
    ///
    /// # Errors
    /// As [`App::after_task_arg`].
    pub(crate) async fn task_number_arg(
        &self,
        caller: &ProjectCaller,
        number: &Value,
        key: &str,
    ) -> AppResult<TaskRecord> {
        let number = number
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| AppError::Invalid(format!("`{key}` must be a task number")))?;
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
        // Before anything is read for the caller: what `after_all_tasks` answers says how
        // many tasks the project has.
        let user = self.manage_authority(caller).await?;
        let all_tasks = if after_all_tasks_arg(args)? {
            Some(self.all_tasks_rule(&caller.sandbox, None).await?)
        } else {
            None
        };
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
            start_rule: match &all_tasks {
                Some(rule) => rule.clone(),
                None => opt_str(args, "start_rule")?,
            },
            supertask: flag_arg(args, "supertask")?,
            prepared: flag_arg(args, "prepared")?,
            start_at: opt_str(args, "start_at")?,
        };
        let now = self.now().0;
        self.tool_tasks
            .reserve(&caller.sandbox, now)
            .map_err(|minutes| {
                AppError::Capacity(format!(
                    "tools may create at most {MAX_TOOL_TASKS_PER_HOUR} tasks an hour in a project; \
                     the next one is possible in {minutes} min"
                ))
            })?;
        // A task an automation's task creates is an automation's too: it holds nobody's
        // personal secrets (SPEC-245 RES-002), though its owner is the automation's creator.
        let origin = if caller
            .task
            .as_ref()
            .is_some_and(|t| crate::vault::Holder::of_task(t) == crate::vault::Holder::Nobody)
        {
            TaskOrigin::Automation
        } else {
            TaskOrigin::Agent
        };
        let task = match self
            .create_task_as(&user, &caller.sandbox, &request, origin)
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
            "note": match all_tasks {
                Some(None) => NOTHING_LEFT,
                _ => "The task is on the Tasks page of this project.",
            },
        }))
    }
}

impl App {
    /// `task_update` (SPEC-396 ATR-006): the UI's change of how a waiting task of the caller's
    /// project starts (SPEC-126), with `task_create`'s start arguments, for the person it acts
    /// for; audited once, with the caller (MGT-010).
    ///
    /// # Errors
    /// `Invalid` arguments, `NotFound` outside the project, `Forbidden` without write access, and
    /// the change's errors (`Conflict` once the task has started).
    pub async fn tool_task_update(
        self: &Arc<Self>,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        let ctx = self.manage_context(caller).await?;
        let task = self.agent_task_ref(caller, args).await?;
        let after_task_id = match args.get("after_task") {
            None | Some(Value::Null) => None,
            Some(number) => Some(self.after_task_arg(caller, number).await?.id),
        };
        let all_tasks = if after_all_tasks_arg(args)? {
            Some(self.all_tasks_rule(&caller.sandbox, Some(&task)).await?)
        } else {
            None
        };
        let change = ChangeStart {
            after_task_id: after_task_id.map(|id| id.as_str().to_owned()),
            start_rule: match &all_tasks {
                Some(rule) => rule.clone(),
                None => opt_str(args, "start_rule")?,
            },
            supertask: flag_arg(args, "supertask")?,
            prepared: flag_arg(args, "prepared")?,
            start_at: opt_str(args, "start_at")?,
        };
        let changed = self
            .change_task_start_as(
                &ctx,
                task.id.as_str(),
                &change,
                Some((&caller.actor, &caller.detail)),
            )
            .await?;
        tracing::info!(task = %task.id, actor = %caller.actor, status = %changed.status, "task start changed by a tool");
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        let mut answer = json!({"task": self.task_brief(&task).await?});
        if all_tasks == Some(None) {
            answer["note"] = json!(NOTHING_LEFT);
        }
        Ok(answer)
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
pub(crate) fn tool_detail(caller: &ProjectCaller, extra: &Value) -> String {
    let mut detail = caller.detail.clone();
    if let (Some(detail), Some(extra)) = (detail.as_object_mut(), extra.as_object()) {
        detail.extend(extra.clone());
    }
    detail.to_string()
}

impl App {
    /// The person a caller acts for, as the UI's functions take them, once they may change
    /// things in its project ([`App::manage_authority`]).
    pub(crate) async fn manage_context(&self, caller: &ProjectCaller) -> AppResult<AuthContext> {
        Ok(AuthContext {
            user: self.manage_authority(caller).await?,
            csrf_token: None,
            session_hash: None,
            token: None,
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
                let attachments = self
                    .delete_task_as(&ctx, id, &caller.actor, Some(&caller.detail))
                    .await?;
                tracing::info!(task = %task.id, actor = %caller.actor, "task deleted by a tool");
                return Ok(json!({"deleted": {
                    "task_id": id,
                    "number": task.number,
                    "attachments": attachments,
                }}));
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
        let ctx = self.configure_context(caller).await?;
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
        let ctx = self.configure_context(caller).await?;
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
        let user = self.configure_authority(caller).await?;
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
