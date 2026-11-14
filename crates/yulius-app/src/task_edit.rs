//! Task control a person had no function for (SPEC-487 slice 3, TCT-001 to TCT-005; wall 2):
//! renaming a task, replacing the request of one that has not started, the order of their
//! queued tasks, closing out an idle task, and one action on several tasks.
//!
//! Contract:
//! - each is a person's function as a page's is: it takes their [`AuthContext`], checks their
//!   write access to the project at the call, and changes only a task they own;
//! - the request of a task is replaced by one conditional update, only while the task has not
//!   started and the request is still the one that was read; the files attached to the old
//!   request stay attached;
//! - the order changes among queued tasks only, which swap the places they hold; a task not
//!   named keeps its place;
//! - a bulk action does to each task what the single function does and reports each task's
//!   outcome; one task's refusal does not stop the others.

use std::sync::Arc;

use serde_json::{Value, json};
use yulius_contracts::api::tasks::TaskSummary;
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::task::{Activity, TaskStatus};

use crate::auth::AuthContext;
use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::error::{AppError, AppResult};
use crate::lifecycle::MAX_PROMPT_BYTES;
use crate::manage::TaskChange;
use crate::model::TaskRecord;
use crate::ports::store::TaskFieldUpdate;

/// The longest title a person gives a task, in characters.
pub const TITLE_MAX_CHARS: usize = 120;

/// The most tasks one call names (`tasks_reorder`, `tasks_bulk`).
pub const TASKS_MAX: usize = 100;

/// What a person changes of a task: its title, its request, or both.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditTask {
    /// The new title.
    pub title: Option<String>,
    /// The new request, which replaces the whole old one.
    pub prompt: Option<String>,
}

/// A task after [`App::edit_task`].
#[derive(Debug, Clone)]
pub struct EditedTask {
    /// The task as it is now.
    pub task: TaskRecord,
    /// The request that was replaced, whole, when one was.
    pub previous_prompt: Option<String>,
}

/// What one call does to several tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkAction {
    /// Stop each, as `task_stop`: a waiting task is cancelled, a running one interrupted.
    Stop,
    /// Pause each, as Pause on its page.
    Pause,
    /// Resume each a person paused.
    Unpause,
    /// Send each the same message, as `task_message`.
    Message,
}

impl BulkAction {
    /// The action named `name`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "stop" => Self::Stop,
            "pause" => Self::Pause,
            "unpause" => Self::Unpause,
            "message" => Self::Message,
            _ => return None,
        })
    }

    /// Its name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Pause => "pause",
            Self::Unpause => "unpause",
            Self::Message => "message",
        }
    }

    /// Whether `all` takes a task in `status` (`paused`: a person paused its run): the tasks
    /// the action can do something to. A task that ended is in none.
    #[must_use]
    pub fn applies(self, status: TaskStatus, paused: bool) -> bool {
        let waiting = matches!(
            status,
            TaskStatus::Queued
                | TaskStatus::Chained
                | TaskStatus::Prepared
                | TaskStatus::WaitingCapacity
        );
        match self {
            Self::Stop => !status.is_resting() && status != TaskStatus::Stopping,
            Self::Pause => status == TaskStatus::Running && !paused,
            Self::Unpause => paused,
            Self::Message => status == TaskStatus::Running || waiting,
        }
    }
}

/// A title as a person gives it: trimmed, 1 to [`TITLE_MAX_CHARS`] characters on one line.
///
/// # Errors
/// `Invalid` otherwise.
pub fn clean_title(title: &str) -> AppResult<String> {
    let title = title.trim();
    let length = title.chars().count();
    if length == 0 || length > TITLE_MAX_CHARS || title.chars().any(char::is_control) {
        return Err(AppError::Invalid(format!(
            "the title must be 1 to {TITLE_MAX_CHARS} characters on one line"
        )));
    }
    Ok(title.to_owned())
}

/// What a task that has started answers to a new request.
fn started(number: u32) -> AppError {
    AppError::Conflict(format!(
        "task #{number} has started, so its request stays as it was read: tell it what \
         changed with task_message"
    ))
}

/// The task numbers of `value`: a list of 1 to [`TASKS_MAX`] numbers, each once.
///
/// # Errors
/// `Invalid` otherwise, naming `key`.
pub fn numbers_arg(value: Option<&Value>, key: &str) -> AppResult<Vec<u32>> {
    let invalid = || {
        AppError::Invalid(format!(
            "`{key}` is a list of 1 to {TASKS_MAX} task numbers"
        ))
    };
    let list = value.and_then(Value::as_array).ok_or_else(invalid)?;
    if list.is_empty() || list.len() > TASKS_MAX {
        return Err(invalid());
    }
    let mut numbers = Vec::with_capacity(list.len());
    for number in list {
        let number = number
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(invalid)?;
        if numbers.contains(&number) {
            return Err(AppError::Invalid(format!(
                "`{key}` names task #{number} twice"
            )));
        }
        numbers.push(number);
    }
    Ok(numbers)
}

impl App {
    /// Renames a task, replaces its request, or both (SPEC-487 TCT-001, TCT-002), for its
    /// owner. The request is replaced only while the task has not started (it is queued,
    /// chained, prepared or waits for capacity, and never ran), whole, and the files attached
    /// to the old one stay attached ([`yulius_domain::request::keeping_attachments`]). The
    /// title is the task's name in Yulius: its branch and its pull request keep theirs. Both
    /// are checked before either is written, and a request that cannot be replaced changes
    /// nothing. Audited under the person's name.
    ///
    /// # Errors
    /// `Invalid` for neither, for a title or a request that is none; `Forbidden` for anyone but
    /// the owner; `Conflict` once the task has started, or when its request changed since it
    /// was read.
    pub async fn edit_task(
        &self,
        ctx: &AuthContext,
        id: &str,
        req: &EditTask,
    ) -> AppResult<EditedTask> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let title = req.title.as_deref().map(clean_title).transpose()?;
        let prompt = match req.prompt.as_deref().map(str::trim) {
            None => None,
            Some(prompt) if prompt.is_empty() || prompt.len() > MAX_PROMPT_BYTES => {
                return Err(AppError::Invalid(
                    "the prompt must be 1 byte to 64 KiB".into(),
                ));
            }
            Some(prompt) => Some(prompt),
        };
        if title.is_none() && prompt.is_none() {
            return Err(AppError::Invalid(
                "give `title` or `prompt` (or both)".into(),
            ));
        }
        let store = &self.deps.store;
        let login = self.login_of(&ctx.user.id).await;
        let now = self.now();
        let mut previous_prompt = None;
        if let Some(prompt) = prompt {
            if self.task_started(&task).await? {
                return Err(started(task.number));
            }
            let request = yulius_domain::request::keeping_attachments(&task.prompt, prompt);
            if request.len() > MAX_PROMPT_BYTES {
                return Err(AppError::Invalid(
                    "the prompt with the files attached to the task must be within 64 KiB".into(),
                ));
            }
            if !store
                .replace_unstarted_prompt(&task.id, &task.prompt, &request, now)
                .await?
            {
                // It started meanwhile, or a message joined its request.
                let current = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
                return Err(if self.task_started(&current).await? {
                    started(task.number)
                } else {
                    AppError::Conflict(format!(
                        "task #{}'s request changed meanwhile: read it again (task_get), then \
                         try again",
                        task.number
                    ))
                });
            }
            let detail = json!({"bytes_before": task.prompt.len(), "bytes": request.len()});
            self.audit_task_edit(&login, "task.request_replaced", &task, &detail)
                .await;
            previous_prompt = Some(task.prompt.clone());
        }
        if let Some(title) = title.filter(|title| *title != task.title) {
            let update = TaskFieldUpdate {
                title: Some(title.clone()),
                ..TaskFieldUpdate::default()
            };
            store.update_task(&task.id, &update, now).await?;
            let detail = json!({"from": task.title, "to": title});
            self.audit_task_edit(&login, "task.renamed", &task, &detail)
                .await;
        }
        self.publish_task(&task.id).await;
        let task = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
        Ok(EditedTask {
            task,
            previous_prompt,
        })
    }

    /// The change happened: a failed audit write is logged, not reported as a failed call
    /// (which would invite doing it twice).
    async fn audit_task_edit(&self, login: &str, action: &str, task: &TaskRecord, detail: &Value) {
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                login,
                action,
                Some(task.id.as_str()),
                Some(&detail.to_string()),
            )
            .await
        {
            tracing::warn!(task = %task.id, error = %err, action, "a task's edit was not audited");
        }
    }

    /// Closes out a running task whose agent is idle (SPEC-487 TCT-004), for its owner: its
    /// session is shut down gracefully and the task ends `closed`, as Yulius closes a finished
    /// task (TSK-014), whatever its plan or pull request say. Background shells it left
    /// running end with the session. A task in a turn is not closed (Stop interrupts it), nor
    /// one Yulius still has pull request news for (it goes on by itself, and tasks chained
    /// after it wait for that, CHN-001); one that waits or has ended is told so
    /// ([`close_refusal`]). Audited under the person's name.
    ///
    /// # Errors
    /// `Forbidden` for anyone but the owner; `Conflict` unless the task runs and is idle.
    pub async fn close_task_for_person(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<TaskSummary> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let running = task.status == TaskStatus::Running;
        let facts = CloseFacts {
            status: task.status,
            activity: task.activity,
            started: self.task_started(&task).await?,
            // A flag can stand while a turn runs (a permission asked mid-turn): the turn
            // decides, not the activity alone.
            turn: running
                && self
                    .existing_runtime(&task.id)
                    .is_some_and(|runtime| runtime.turn_in_progress()),
            news: running
                && (self.agent_waits_on_yulius(&task.id) || self.pr_news_due(&task).await),
        };
        if let Some(why) = close_refusal(task.number, facts) {
            return Err(AppError::Conflict(why));
        }
        self.close_task_now(&task.id).await?;
        let login = self.login_of(&ctx.user.id).await;
        self.audit_task_edit(&login, "task.closed_by_person", &task, &json!({}))
            .await;
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.task_summary(&task).await
    }

    /// Puts the person's queued tasks `numbers` of `sandbox` in that order (SPEC-487 TCT-003):
    /// they take the places they hold among themselves, the first the earliest; every other
    /// task keeps its place. Queued tasks start in the queue's order as sessions get free (a
    /// supertask's hold comes before the order, SPEC-150). Answers the sandbox's queue after
    /// the change.
    ///
    /// # Errors
    /// `Invalid` for fewer than two tasks or a number that is no task; `Forbidden` for another
    /// person's task; `Conflict` when one is not queued (nothing changes).
    pub async fn reorder_queue(
        &self,
        ctx: &AuthContext,
        sandbox: &SandboxId,
        numbers: &[u32],
    ) -> AppResult<Vec<TaskRecord>> {
        self.require_for(ctx, sandbox, Action::Work).await?;
        if numbers.len() < 2 {
            return Err(AppError::Invalid(
                "an order needs at least two queued tasks".into(),
            ));
        }
        let store = &self.deps.store;
        let mut order = Vec::with_capacity(numbers.len());
        for number in numbers {
            let task = store
                .task_by_number(sandbox, *number)
                .await?
                .ok_or_else(|| AppError::Invalid(format!("no task #{number} in this project")))?;
            self.require_owner(ctx, &task).await?;
            if task.status != TaskStatus::Queued {
                return Err(not_queued(&task));
            }
            order.push(task.id);
        }
        if !store.reorder_queued(&order, self.now()).await? {
            // One of them left the queue since it was read.
            for id in &order {
                if let Some(task) = store.task(id).await?
                    && task.status != TaskStatus::Queued
                {
                    return Err(not_queued(&task));
                }
            }
            return Err(AppError::Conflict(
                "the queue changed meanwhile: try again".into(),
            ));
        }
        let login = self.login_of(&ctx.user.id).await;
        if let Err(err) = store
            .audit(
                self.now(),
                &login,
                "tasks.reordered",
                Some(sandbox.as_str()),
                Some(&json!({"order": numbers}).to_string()),
            )
            .await
        {
            tracing::warn!(sandbox = %sandbox, error = %err, "a queue order was not audited");
        }
        for id in &order {
            self.publish_task(id).await;
        }
        Ok(store
            .tasks_with_status(&[TaskStatus::Queued])
            .await?
            .into_iter()
            .filter(|task| task.sandbox_id == *sandbox)
            .collect())
    }

    /// Does `action` to several tasks of the caller's project for the person it acts for
    /// (SPEC-487 TCT-005): the tasks `numbers`, or with none every task of the person's own
    /// that the action applies to ([`BulkAction::applies`]). Each task gets what the single
    /// tool does, with its checks and its audit, one after the other; the answer names the
    /// tasks done, the tasks refused with the reason, and how many tasks of other people `all`
    /// left alone. Never the caller's own task.
    ///
    /// # Errors
    /// `Forbidden` without write access; store failures. A task's own refusal is in the answer.
    pub async fn bulk_tasks(
        self: &Arc<Self>,
        ctx: &AuthContext,
        caller: &ProjectCaller,
        action: BulkAction,
        numbers: Option<&[u32]>,
        text: Option<&str>,
    ) -> AppResult<Value> {
        self.require_for(ctx, &caller.sandbox, Action::Work).await?;
        let store = &self.deps.store;
        let (mut others, mut beyond) = (0_usize, 0_usize);
        let targets: Vec<Result<TaskRecord, u32>> = match numbers {
            Some(numbers) => {
                let mut targets = Vec::with_capacity(numbers.len());
                for number in numbers {
                    targets.push(
                        store
                            .task_by_number(&caller.sandbox, *number)
                            .await?
                            .ok_or(*number),
                    );
                }
                targets
            }
            None => {
                let mut targets = Vec::new();
                for task in store.tasks(&caller.sandbox, false).await? {
                    let own = caller.task.as_ref().is_some_and(|own| own.id == task.id);
                    // A person's pause is of a live run: a task that ended has none.
                    let paused =
                        !task.status.is_resting() && self.is_paused_by_person(&task.id).await;
                    if own || !action.applies(task.status, paused) {
                        continue;
                    }
                    if task.owner == ctx.user.id {
                        targets.push(Ok(task));
                    } else {
                        others += 1;
                    }
                }
                targets.sort_by_key(|task| task.as_ref().map_or(0, |task| task.number));
                targets
            }
        };
        // A stop takes the waiting tasks first, and the bound cuts what comes last.
        let mut targets = if action == BulkAction::Stop {
            waiting_first(targets, |target| {
                target.as_ref().ok().map(|task| task.status)
            })
        } else {
            targets
        };
        if numbers.is_none() {
            beyond = targets.len().saturating_sub(TASKS_MAX);
            targets.truncate(TASKS_MAX);
        }
        let (mut done, mut refused) = (Vec::new(), Vec::new());
        for target in targets {
            let task = match target {
                Ok(task) => task,
                Err(number) => {
                    refused.push(json!({
                        "number": number,
                        "why": format!("no task #{number} in this project"),
                    }));
                    continue;
                }
            };
            let id = task.id.as_str();
            let by_id = json!({"task_id": id});
            let outcome = match action {
                BulkAction::Stop => self
                    .tool_task_change(caller, &by_id, TaskChange::Stop)
                    .await
                    .map(|_| json!({})),
                BulkAction::Pause => self.pause_task_for_person(ctx, id).await.map(|_| json!({})),
                // Resume answers a task nobody paused as it is: said here, so the answer
                // does not list as resumed what was never paused.
                BulkAction::Unpause if !self.is_paused_by_person(&task.id).await => Err(
                    AppError::Conflict(format!("task #{} is not paused by a person", task.number)),
                ),
                BulkAction::Unpause => self
                    .resume_task_for_person(ctx, id)
                    .await
                    .map(|_| json!({})),
                BulkAction::Message => {
                    let args = json!({"task_id": id, "text": text.unwrap_or_default()});
                    self.tool_task_message(caller, &args)
                        .await
                        .map(|answer| json!({"delivered": answer["delivered"]}))
                }
            };
            match outcome {
                Ok(mut entry) => {
                    entry["number"] = json!(task.number);
                    entry["title"] = json!(task.title);
                    done.push(entry);
                }
                Err(err) => refused.push(json!({
                    "number": task.number,
                    "title": task.title,
                    "why": err.public_message(),
                })),
            }
        }
        let mut answer = json!({"do": action.as_str(), "done": done, "refused": refused});
        if numbers.is_none() {
            answer["others_left_alone"] = json!(others);
        }
        if beyond > 0 {
            // One call does at most `TASKS_MAX`: the rest is said, never dropped silently.
            answer["not_reached"] = json!(beyond);
            answer["note"] = json!(format!(
                "{beyond} more task(s) were not reached (at most {TASKS_MAX} a call): call again"
            ));
        }
        Ok(answer)
    }
}

/// The tasks of a bulk stop in the order they are stopped: those that wait to start first,
/// then the others, each group as it was. A session that gets free starts the next queued
/// task at once, so stopping the running ones first would start tasks only to stop them.
fn waiting_first<T>(mut targets: Vec<T>, status: impl Fn(&T) -> Option<TaskStatus>) -> Vec<T> {
    targets.sort_by_key(|target| {
        !matches!(
            status(target),
            Some(
                TaskStatus::Queued
                    | TaskStatus::Chained
                    | TaskStatus::Prepared
                    | TaskStatus::WaitingCapacity
            )
        )
    });
    targets
}

/// What a task is doing when its person asks to close it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloseFacts {
    /// Its status.
    pub status: TaskStatus,
    /// Its agent's activity.
    pub activity: Option<Activity>,
    /// It has had a run or a session (a task held for capacity at a turn's end has).
    pub started: bool,
    /// A turn of its session is known to be in progress.
    pub turn: bool,
    /// Yulius has pull request news for it still to come, or its agent waits for some.
    pub news: bool,
}

/// Why task #`number` is not closed out, when it is not: only a running task whose agent is
/// idle and that Yulius has no pull request news for is (SPEC-487 TCT-004).
#[must_use]
pub fn close_refusal(number: u32, facts: CloseFacts) -> Option<String> {
    let what = facts.status.as_str();
    match facts.status {
        // A running task without an activity yet is in its first turn, as the lists show it.
        TaskStatus::Running
            if facts.turn || matches!(facts.activity, Some(Activity::Working) | None) =>
        {
            Some(format!(
                "task #{number} is working: wait for its turn to end, or stop it (task_stop)"
            ))
        }
        TaskStatus::Running if facts.news => Some(format!(
            "Yulius still has news of its pull request for task #{number} (a merge is armed, or \
             its merge is not told yet): it goes on by itself, and tasks chained after it wait \
             for that. Stop it (task_stop) to end it anyway"
        )),
        TaskStatus::Running => None,
        status if status.is_resting() => Some(format!(
            "task #{number} has ended ({what}): nothing is left to close"
        )),
        TaskStatus::Queued
        | TaskStatus::Chained
        | TaskStatus::Prepared
        | TaskStatus::WaitingCapacity
            if !facts.started =>
        {
            Some(format!(
                "task #{number} has not started ({what}): task_stop cancels it"
            ))
        }
        TaskStatus::Queued | TaskStatus::WaitingCapacity => Some(format!(
            "task #{number} is {what} and goes on by itself: it has no idle session to close; \
             task_stop stops it"
        )),
        _ => Some(format!("task #{number} is {what}: try again once it runs")),
    }
}

/// What a task that is not queued answers to a place in the queue.
fn not_queued(task: &TaskRecord) -> AppError {
    AppError::Conflict(format!(
        "task #{} is not queued ({}): only tasks that wait for a free session have a place in \
         the queue; how a chained or prepared task starts is changed with task_update",
        task.number,
        task.status.as_str()
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        BulkAction, CloseFacts, TASKS_MAX, TITLE_MAX_CHARS, clean_title, close_refusal, numbers_arg,
    };
    use serde_json::json;
    use yulius_domain::task::Activity;
    use yulius_domain::task::{StopReason, TaskStatus};

    /// SPEC-487 TCT-001: a title is one line of 1 to 120 characters, trimmed.
    #[test]
    fn a_title_is_one_bounded_line() {
        assert_eq!(
            clean_title("  Footer fix \t").ok(),
            Some("Footer fix".into())
        );
        let longest = "ł".repeat(TITLE_MAX_CHARS);
        assert_eq!(clean_title(&longest).ok(), Some(longest.clone()));
        for refused in ["", "   ", "two\nlines", "bell\u{7}", &format!("{longest}x")] {
            assert!(clean_title(refused).is_err(), "{refused:?}");
        }
    }

    /// SPEC-487 TCT-005: `all` takes the tasks an action can do something to, and never one
    /// that ended.
    #[test]
    fn all_takes_the_tasks_an_action_applies_to() {
        use TaskStatus::{
            Chained, Closed, Prepared, Queued, Running, Starting, Stopping, Switching,
            WaitingCapacity,
        };
        let ended = [Closed, TaskStatus::Stopped(StopReason::Cancelled)];
        for action in [
            BulkAction::Stop,
            BulkAction::Pause,
            BulkAction::Message,
            BulkAction::Unpause,
        ] {
            for status in ended {
                assert!(!action.applies(status, false), "{action:?} {status:?}");
            }
            assert_eq!(BulkAction::parse(action.as_str()), Some(action));
        }
        assert_eq!(BulkAction::parse("delete"), None);
        for status in [
            Running,
            Starting,
            Switching,
            Queued,
            Chained,
            Prepared,
            WaitingCapacity,
        ] {
            assert!(BulkAction::Stop.applies(status, false), "{status:?}");
        }
        assert!(!BulkAction::Stop.applies(Stopping, false));
        assert!(BulkAction::Pause.applies(Running, false));
        assert!(!BulkAction::Pause.applies(Running, true), "paused already");
        for status in [Starting, Queued, Chained, Prepared, WaitingCapacity] {
            assert!(!BulkAction::Pause.applies(status, false), "{status:?}");
        }
        assert!(BulkAction::Unpause.applies(Running, true));
        assert!(!BulkAction::Unpause.applies(Running, false));
        for status in [Running, Queued, Chained, Prepared, WaitingCapacity] {
            assert!(BulkAction::Message.applies(status, false), "{status:?}");
        }
        for status in [Starting, Switching, Stopping] {
            assert!(!BulkAction::Message.applies(status, false), "{status:?}");
        }
    }

    /// SPEC-487 TCT-004 (review round 1, F2 and F3): only a running task whose agent is idle
    /// is closed out. A turn in progress counts whatever the activity reads (a flag can stand
    /// mid-turn); a task Yulius has pull request news for goes on by itself; a task held for
    /// capacity that has started is not told it has not.
    #[test]
    fn only_an_idle_running_task_is_closed_out() {
        let facts = |status, activity| CloseFacts {
            status,
            activity,
            started: true,
            turn: false,
            news: false,
        };
        let running = |activity| facts(TaskStatus::Running, activity);
        for idle in [Activity::Waiting, Activity::Done, Activity::NeedsAttention] {
            assert_eq!(close_refusal(7, running(Some(idle))), None, "{idle:?}");
            // Mid-turn, whatever the activity reads.
            let turn = CloseFacts {
                turn: true,
                ..running(Some(idle))
            };
            let said = close_refusal(7, turn).unwrap_or_default();
            assert!(
                said.contains("is working") && said.contains("task_stop"),
                "{said}"
            );
            // News of its pull request still to come.
            let news = CloseFacts {
                news: true,
                ..running(Some(idle))
            };
            let said = close_refusal(7, news).unwrap_or_default();
            assert!(
                said.contains("goes on by itself") && said.contains("chained after it wait"),
                "{said}"
            );
        }
        for working in [Some(Activity::Working), None] {
            let said = close_refusal(7, running(working)).unwrap_or_default();
            assert!(said.contains("is working"), "{working:?}: {said}");
        }
        for ended in [
            TaskStatus::Closed,
            TaskStatus::Stopped(StopReason::Cancelled),
        ] {
            let said = close_refusal(7, facts(ended, None)).unwrap_or_default();
            assert!(said.contains("has ended"), "{said}");
        }
        for waiting in [
            TaskStatus::Queued,
            TaskStatus::Chained,
            TaskStatus::Prepared,
            TaskStatus::WaitingCapacity,
        ] {
            let never = CloseFacts {
                started: false,
                ..facts(waiting, None)
            };
            let said = close_refusal(7, never).unwrap_or_default();
            assert!(
                said.contains("has not started") && said.contains("task_stop cancels it"),
                "{said}"
            );
        }
        for held in [TaskStatus::Queued, TaskStatus::WaitingCapacity] {
            let said = close_refusal(7, facts(held, None)).unwrap_or_default();
            assert!(
                said.contains("goes on by itself") && !said.contains("has not started"),
                "{said}"
            );
        }
        for between in [
            TaskStatus::Starting,
            TaskStatus::Switching,
            TaskStatus::Stopping,
        ] {
            let said = close_refusal(7, facts(between, None)).unwrap_or_default();
            assert!(said.contains("try again once it runs"), "{said}");
        }
    }

    /// SPEC-487 TCT-005: a bulk stop cancels the tasks that wait to start before it stops
    /// the running ones, so no session it frees starts a task it is about to stop.
    #[test]
    fn a_bulk_stop_takes_the_waiting_tasks_first() {
        use TaskStatus::{Chained, Prepared, Queued, Running, Starting, WaitingCapacity};
        let tasks = vec![
            (1, Some(Running)),
            (2, Some(Queued)),
            (3, None),
            (4, Some(Starting)),
            (5, Some(Prepared)),
            (6, Some(Chained)),
            (7, Some(WaitingCapacity)),
        ];
        let order: Vec<u32> = super::waiting_first(tasks, |task| task.1)
            .into_iter()
            .map(|task| task.0)
            .collect();
        assert_eq!(order, [2, 5, 6, 7, 1, 3, 4]);
    }

    /// SPEC-487 TCT-003, TCT-005: a call names 1 to 100 tasks, each once, by number.
    #[test]
    fn a_call_names_a_bounded_list_of_tasks_each_once() {
        assert_eq!(
            numbers_arg(Some(&json!([3, 1])), "numbers").ok(),
            Some(vec![3, 1])
        );
        let most: Vec<usize> = (1..=TASKS_MAX).collect();
        assert_eq!(
            numbers_arg(Some(&json!(most)), "numbers")
                .map(|n| n.len())
                .ok(),
            Some(TASKS_MAX)
        );
        let too_many: Vec<usize> = (1..=TASKS_MAX + 1).collect();
        for refused in [
            json!([]),
            json!(too_many),
            json!("all"),
            json!([1, "2"]),
            json!([1, -2]),
            json!([1, 4_294_967_296_u64]),
            json!([1.5]),
        ] {
            let said = numbers_arg(Some(&refused), "numbers")
                .err()
                .map(|e| e.public_message());
            assert!(
                said.as_deref()
                    .is_some_and(|said| said.contains("`numbers`") && said.contains("100")),
                "{refused}: {said:?}"
            );
        }
        assert!(numbers_arg(None, "numbers").is_err());
        let twice = numbers_arg(Some(&json!([2, 5, 2])), "numbers").err();
        assert!(
            twice.is_some_and(|e| e.public_message().contains("#2 twice")),
            "a task named twice"
        );
    }
}
