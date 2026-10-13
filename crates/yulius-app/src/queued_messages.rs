//! Queued messages (SPEC-052): a person corrects a task that has not started yet. Each message is
//! added to the task's request — the prompt every reader of the task's ask uses: the request file,
//! the goal, the first message, the scope check, `task_get` and the export — so the agent gets it
//! when the task starts. Once the task has a run or a session, Claude Code's own input takes typed
//! messages and the request is never read again: a message is refused then.

use yulius_contracts::api::tasks::{QueueMessage, QueuedMessage};
use yulius_domain::permission::Action;
use yulius_domain::request::queued_addition;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::lifecycle::MAX_PROMPT_BYTES;

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
        if task.prompt.len() + addition.len() > MAX_PROMPT_BYTES {
            return Err(too_long());
        }
        let store = &self.deps.store;
        if !store
            .append_unstarted_prompt(&task.id, &addition, MAX_PROMPT_BYTES, now)
            .await?
        {
            // Another message may have filled the request meanwhile; else the task started.
            let current = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
            if current.prompt.len() + addition.len() > MAX_PROMPT_BYTES {
                return Err(too_long());
            }
            return Err(AppError::Conflict(
                "The task has started: type into its session instead".into(),
            ));
        }
        self.publish_task(&task.id).await;
        let task = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
        Ok(QueuedMessage {
            request: task.prompt,
        })
    }
}
