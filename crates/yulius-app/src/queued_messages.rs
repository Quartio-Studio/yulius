//! Queued messages (SPEC-052): a person corrects a task that has not started yet. Each message is
//! added to the task's request — the prompt every reader of the task's ask uses: the request file,
//! the goal, the first message, the scope check, `task_get` and the export — so the agent gets it
//! when the task starts. Once the task has a run or a session, Claude Code's own input takes typed
//! messages and the request is never read again: a message is refused then.

use yulius_contracts::api::tasks::{QueueMessage, QueuedMessage};
use yulius_domain::permission::Action;
use yulius_domain::request::queued_addition;
use yulius_domain::task::TaskStatus;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::lifecycle::MAX_PROMPT_BYTES;
use crate::model::TaskRecord;

impl App {
    /// `POST /api/tasks/{tid}/queued-messages` (QMS-001..003): adds the message to the request
    /// of a task that has never started and announces the change.
    ///
    /// # Errors
    /// `Invalid` for an empty message or a request past [`MAX_PROMPT_BYTES`]; `Conflict` once the
    /// task has started; access and store failures.
    pub async fn queue_task_message(
        &self,
        ctx: &AuthContext,
        id: &str,
        req: &QueueMessage,
    ) -> AppResult<QueuedMessage> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let login = self.login_of(&ctx.user.id).await;
        let now = self.now();
        // "2026-10-13 10:42 UTC": minutes are enough to order a person's corrections.
        let at = format!(
            "{} UTC",
            rfc3339(now).replace('T', " ").get(..16).unwrap_or_default()
        );
        let addition = queued_addition(&login, &at, &req.text)
            .ok_or_else(|| AppError::Invalid("the message is empty".into()))?;
        let too_long = || {
            AppError::Invalid(format!(
                "the request would be longer than {} KiB: start the task and type the rest \
                 into its session",
                MAX_PROMPT_BYTES / 1024
            ))
        };
        // Whether the task started decides first: its session is the way in, whatever the size.
        // The conditional update below is what makes the answer exact; this only words it.
        let refusal = |task: &TaskRecord, started: bool| {
            if started {
                AppError::Conflict("The task has started: type into its session instead".into())
            } else if task.prompt.len() + addition.len() > MAX_PROMPT_BYTES {
                too_long()
            } else {
                AppError::Conflict("The task changed meanwhile: try again".into())
            }
        };
        let started = self.task_started(&task).await?;
        if started || task.prompt.len() + addition.len() > MAX_PROMPT_BYTES {
            return Err(refusal(&task, started));
        }
        let store = &self.deps.store;
        if !store
            .append_unstarted_prompt(&task.id, &addition, MAX_PROMPT_BYTES, now)
            .await?
        {
            // It started meanwhile, or another message filled the request.
            let current = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
            return Err(refusal(&current, self.task_started(&current).await?));
        }
        self.publish_task(&task.id).await;
        let task = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
        Ok(QueuedMessage {
            request: task.prompt,
        })
    }

    /// Whether a task is past taking queued messages: it is no longer waiting, or it has had a
    /// run or a provider session (QMS-001; the store's conditional update holds the same rule).
    async fn task_started(&self, task: &TaskRecord) -> AppResult<bool> {
        let waiting = matches!(
            task.status,
            TaskStatus::Queued | TaskStatus::Chained | TaskStatus::WaitingCapacity
        );
        let store = &self.deps.store;
        Ok(!waiting
            || !store.attempts(&task.id).await?.is_empty()
            || !store.provider_sessions(&task.id).await?.is_empty())
    }
}
