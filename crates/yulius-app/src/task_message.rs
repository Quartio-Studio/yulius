//! Messages to tasks (SPEC-230, ADR-0178): a person's Superagent tells a task what the person
//! said, with `task_message`.
//!
//! Contract:
//! - a task that has not started takes the message into its request, as the task view's queued
//!   message of the Superagent's person (SPEC-052, TMS-002);
//! - a running task gets it typed into its session as one bracketed paste, labelled with the
//!   person, then a separate Enter; Claude Code queues a message submitted while the agent works
//!   (TMS-003). Control characters other than line breaks and tabs never reach the TUI;
//! - nothing is typed while a person typed in the last minute (their draft is theirs), while
//!   Claude Code asks a person whether to trust the folder, while Yulius types the first message,
//!   or while another message is typed; other states are refused with their name (TMS-004);
//! - the person needs write access; every message is audited `task.messaged` without its text
//!   (TMS-005); a typed message ends the task's open `needs_you` (TMS-006).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use yulius_contracts::api::tasks::QueueMessage;
use yulius_domain::task::TaskStatus;

use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::error::{AppError, AppResult};
use crate::lifecycle::MAX_PROMPT_BYTES;
use crate::manage::tool_detail;
use crate::model::TaskRecord;
use crate::supervisor::trust_prompt_shown;

/// Longest message, in bytes after cleaning.
pub const MAX_MESSAGE_BYTES: usize = MAX_PROMPT_BYTES;

/// The wait between the pasted text and its Enter, so the TUI sees typed text and then a submit
/// (as for the goal and `/compact`).
const SUBMIT_DELAY: Duration = Duration::from_millis(300);

/// The text as it is typed: line breaks as LF, every other control character but tab removed
/// (an ESC would end the bracketed paste or drive the TUI), surrounding whitespace trimmed.
#[must_use]
pub fn clean_message(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    text.chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect::<String>()
        .trim()
        .to_owned()
}

/// The bytes typed for a (cleaned) message from `login`: one bracketed paste of its label and
/// text. Enter is sent separately.
#[must_use]
pub fn message_paste(login: &str, text: &str) -> Vec<u8> {
    let login = clean_message(login);
    format!("\x1b[200~Message from {login} (through their Superagent):\n\n{text}\x1b[201~")
        .into_bytes()
}

/// Clears a run's "a message is being typed" mark when the typing ends, even if the call is
/// dropped half-way.
struct Typing<'a>(&'a AtomicBool);

impl Drop for Typing<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl App {
    /// `task_message {task_id | number, text}` (SPEC-230): the Superagent's message to a task of
    /// its project, for the person it acts for: queued into the request of a task that has not
    /// started, typed into the session of a running one.
    ///
    /// # Errors
    /// `Invalid` for an empty or oversized text; `NotFound` outside the project; `Forbidden`
    /// without write access or on the caller's own task; `Conflict` when the task cannot take a
    /// message now (the reason says why).
    pub async fn tool_task_message(
        self: &Arc<Self>,
        caller: &ProjectCaller,
        args: &Value,
    ) -> AppResult<Value> {
        let ctx = self.manage_context(caller).await?;
        let task = self.agent_task_ref(caller, args).await?;
        if caller.task.as_ref().is_some_and(|own| own.id == task.id) {
            return Err(AppError::Forbidden("a task cannot message itself".into()));
        }
        let text = args
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Invalid("missing string argument `text`".into()))?;
        let text = clean_message(text);
        if text.is_empty() {
            return Err(AppError::Invalid("the message is empty".into()));
        }
        if text.len() > MAX_MESSAGE_BYTES {
            return Err(AppError::Invalid(format!(
                "the message is longer than {} KiB",
                MAX_MESSAGE_BYTES / 1024
            )));
        }
        let delivered = if task.status == TaskStatus::Running {
            let login = self.login_of(&caller.user).await;
            self.type_task_message(&task, &login, &text).await?;
            "sent"
        } else if !self.task_started(&task).await? {
            self.queue_task_message(&ctx, task.id.as_str(), &QueueMessage { text })
                .await?;
            "queued"
        } else {
            return Err(AppError::Conflict(format!(
                "task #{} is {}, so it takes no message now: {}",
                task.number,
                task.status.as_str(),
                not_now_advice(task.status)
            )));
        };
        let named = json!({
            "task": format!("#{} {}", task.number, task.title),
            "delivered": delivered,
        });
        // The message went out: a failed audit write is logged, not reported as a failed call
        // (which would invite sending it twice).
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &caller.actor,
                "task.messaged",
                Some(task.id.as_str()),
                Some(&tool_detail(caller, &named)),
            )
            .await
        {
            tracing::warn!(task = %task.id, error = %err, "task message audit failed");
        }
        tracing::info!(task = %task.id, actor = %caller.actor, delivered, "task messaged by a tool");
        let note = match delivered {
            "sent" => "Typed into the task's session: its agent reads it as soon as it can.",
            _ => "Added to the task's request: its agent reads it when the task starts.",
        };
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        Ok(json!({
            "task": self.task_brief(&task).await?,
            "delivered": delivered,
            "note": note,
        }))
    }

    /// Types a cleaned message into the running task's session (TMS-003, TMS-004, TMS-006).
    async fn type_task_message(&self, task: &TaskRecord, login: &str, text: &str) -> AppResult<()> {
        let runtime = self.existing_runtime(&task.id).ok_or_else(|| {
            AppError::Conflict(format!("task #{} has no live session", task.number))
        })?;
        if runtime.typing_opening.load(Ordering::SeqCst) {
            return Err(AppError::Conflict(format!(
                "Yulius is still typing task #{}'s first message: try again in a minute",
                task.number
            )));
        }
        // Only a person answers Claude Code's folder-trust dialog (SPEC-117 ONE-005): a paste and
        // an Enter would answer it with its default and lose the message.
        if !runtime.session_started.load(Ordering::SeqCst)
            && trust_prompt_shown(&runtime.hub.screen_text())
        {
            return Err(AppError::Conflict(format!(
                "task #{} waits for a person to answer Claude Code's folder-trust question in its \
                 terminal; tell the person, then try again",
                task.number
            )));
        }
        if self.typed_recently(&task.id).is_some() {
            return Err(AppError::Conflict(format!(
                "a person typed into task #{}'s terminal in the last minute and may have a draft \
                 there: try again later, or tell the person",
                task.number
            )));
        }
        if runtime.typing_message.swap(true, Ordering::SeqCst) {
            return Err(AppError::Conflict(format!(
                "another message to task #{} is being typed: try again in a moment",
                task.number
            )));
        }
        let _typing = Typing(&runtime.typing_message);
        self.send_input(&task.id, &message_paste(login, text))
            .await?;
        tokio::time::sleep(SUBMIT_DELAY).await;
        // The text is in the prompt box but not sent: said, so a retry does not send it twice.
        self.send_input(&task.id, b"\r").await.map_err(|err| {
            AppError::Conflict(format!(
                "the message was pasted into task #{}'s session but not submitted ({}): it may \
                 still be in its prompt box, so look before sending it again",
                task.number,
                err.public_message()
            ))
        })?;
        // The person spoke to the task through their Superagent: what it asked them is answered.
        self.end_needs_you(&task.id).await;
        Ok(())
    }
}

/// What to do instead, for a task that takes no message in `status`.
fn not_now_advice(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Starting | TaskStatus::Switching => "try again once it runs",
        TaskStatus::WaitingCapacity => {
            "it is paused for capacity; try again once it runs, or tell the person"
        }
        TaskStatus::Stopping => "it is stopping",
        _ => "continue the task first, or create a new task with the message",
    }
}

#[cfg(test)]
mod tests {
    use super::{clean_message, message_paste};

    #[test]
    fn escape_sequences_and_control_characters_never_reach_the_session() {
        assert_eq!(
            clean_message("  Use it.\r\nUp to $250 \x1b[201~ now\x07\x00\r\tok \n"),
            "Use it.\nUp to $250 [201~ now\n\tok"
        );
        assert_eq!(clean_message("\x1b\x07 \n"), "");
    }

    #[test]
    fn a_message_is_one_labelled_bracketed_paste() {
        let paste =
            String::from_utf8(message_paste("ali\x1bce", "Go on.\nThanks")).unwrap_or_default();
        assert_eq!(
            paste,
            "\x1b[200~Message from alice (through their Superagent):\n\nGo on.\nThanks\x1b[201~"
        );
        assert_eq!(paste.matches('\x1b').count(), 2);
    }
}
