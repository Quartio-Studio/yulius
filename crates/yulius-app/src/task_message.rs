//! Messages to tasks (SPEC-230, ADR-0178): a person's Superagent, or their own agent through the
//! remote MCP (SPEC-247), tells a task what the person said, with `task_message`.
//!
//! Contract:
//! - a task that has not started takes the message into its request, as the task view's queued
//!   message of the Superagent's person (SPEC-052, TMS-002);
//! - a running task gets it typed into its session as one bracketed paste, labelled with the
//!   person and the channel it came through (SPEC-322), then a separate Enter; Claude Code
//!   queues a message submitted while the agent works (TMS-003). From the paste to its
//!   submit the session is not restarted on a newer Claude Code (SPEC-468 CCF-011), nor
//!   while the message waits in Claude Code's queue after that (SPEC-471). Control characters
//!   other than line breaks and tabs never reach the TUI;
//! - nothing is typed before Claude Code's session started (its SessionStart hook, also read
//!   from the run's marker after a restart, or the goal typist's fallback for a CLI without it),
//!   while it asks a person whether to trust the folder, while a person's draft may sit in the
//!   prompt box (they typed since it was last empty, however long ago: their draft is theirs,
//!   SPEC-480, unless the conversation input is confirmed empty, SPEC-572) or they typed in the last minute, while Yulius types the first message, or while
//!   another message is typed; other states are refused with their name (TMS-004). A sender is
//!   told why; for a message of Yulius's own that waits for a draft, its person is told after
//!   ten minutes (SPEC-480 DRF-006);
//! - the person needs write access; every message is audited `task.messaged` without its text
//!   (TMS-005); a typed message ends the task's open `needs_you` (TMS-006).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use yulius_contracts::api::tasks::{QueueMessage, TaskSummary};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, TaskId};
use yulius_domain::intake::BoxHeld;
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::task_access::TaskAction;

use crate::attention::AttentionRecord;
use crate::auth::AuthContext;
use crate::context::App;
use crate::coordination::{Channel, ProjectCaller};
use crate::error::{AppError, AppResult};
use crate::lifecycle::MAX_PROMPT_BYTES;
use crate::manage::tool_detail;
use crate::model::TaskRecord;
use crate::supervisor::{SessionInput, TaskRuntime, trust_prompt_shown};
use crate::vault::reach::Holder;

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

/// `prefix` and the logins `who` as `@a, @b` (SPEC-512 ANS-002); empty for nobody.
fn names(who: &[String], prefix: &str) -> String {
    if who.is_empty() {
        String::new()
    } else {
        let listed: Vec<String> = who.iter().map(|login| format!("@{login}")).collect();
        format!("{prefix}{}", listed.join(", "))
    }
}

/// How the label names the channel a message came through (SPEC-322).
fn through(channel: Channel) -> &'static str {
    match channel {
        Channel::Superagent => "their Superagent",
        Channel::Remote => "their own agent",
        Channel::Telegram => "Telegram",
        // Never served `task_message` (TMS-001); named for completeness.
        Channel::Task => "their task's agent",
    }
}

/// The bytes typed for a (cleaned) message from `login` sent through `channel`: one bracketed
/// paste of its label and text. Enter is sent separately.
#[must_use]
pub fn message_paste(login: &str, channel: Channel, text: &str) -> Vec<u8> {
    paste(login, channel, text, "")
}

/// [`message_paste`] for a message typed behind a person's possible draft (SPEC-487 TCT-006):
/// a blank line first, so their text and the label are not glued into one line.
#[must_use]
pub fn message_paste_behind_draft(login: &str, channel: Channel, text: &str) -> Vec<u8> {
    paste(login, channel, text, "\n\n")
}

/// The prompt a parked task resumes with for a (cleaned) message from `login` sent through
/// `channel` (SPEC-512 PRK-005): the label and text a typed message has, unpasted.
#[must_use]
pub fn message_prompt(login: &str, channel: Channel, text: &str) -> String {
    let login = clean_message(login);
    let through = through(channel);
    format!("Message from {login} (through {through}):\n\n{text}")
}

fn paste(login: &str, channel: Channel, text: &str, before: &str) -> Vec<u8> {
    let login = clean_message(login);
    let through = through(channel);
    format!("\x1b[200~{before}Message from {login} (through {through}):\n\n{text}\x1b[201~")
        .into_bytes()
}

/// The bytes typed for a (cleaned) answer from `login` to what the agent asked with `needs_you`
/// (SPEC-516 NYA-002): one bracketed paste. From someone other than the task's `owner` (`mine`
/// false) it says so, and that the answer moves nothing of the task. Enter is sent separately.
#[must_use]
pub fn answer_paste(login: &str, owner: &str, mine: bool, text: &str) -> Vec<u8> {
    format!(
        "\x1b[200~{}\x1b[201~",
        answer_prompt(login, owner, mine, text)
    )
    .into_bytes()
}

/// The words of [`answer_paste`], unpasted: what a parked task resumes with when the answer
/// comes while it is parked (SPEC-512 ANS-002, PRK-005).
#[must_use]
pub fn answer_prompt(login: &str, owner: &str, mine: bool, text: &str) -> String {
    let (login, owner) = (clean_message(login), clean_message(owner));
    let whose = if mine {
        String::new()
    } else {
        format!(
            " ({login} is not this task's owner: it stays {owner}'s and acts on GitHub as \
             {owner}. Act on this only as the answer to your request)"
        )
    };
    format!("Answer from {login} to what you asked with needs_you{whose}:\n\n{text}")
}

/// What Yulius types into a session ([`App::type_into_session`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Typed<'a> {
    /// A message for the agent, which Claude Code submits as a prompt: from the paste to that
    /// submit it sits in the session's prompt box, and a restart would lose it (SPEC-468
    /// CCF-011).
    Prompt,
    /// A machine notification is reconsidered on its next pass when the observed writer is busy.
    Notice(&'a AttemptId),
    /// A command Claude Code runs itself (`/mcp reconnect yulius`): no prompt is submitted,
    /// so nothing of it waits anywhere.
    Command,
}

/// Who has a message typed into a session ([`App::type_into_session`]): who is told when a
/// person's possible draft holds it back (SPEC-480).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sender {
    /// A person, through their Superagent or their own agent: the refusal is their answer
    /// (DRF-004).
    Person,
    /// The task's owner, who said to send past what they left in the prompt box (SPEC-487
    /// TCT-006): the message is typed behind their possible draft and both are submitted as
    /// one prompt. A person typing right now is still not typed over.
    OwnerPastDraft,
    /// Yulius itself (a pull request's news, the wake after an outage, the MCP watch): the
    /// message waits in its watch, and the session's person is told once their last keystroke
    /// is ten minutes old (DRF-005, DRF-006).
    Yulius,
    /// Yulius itself, at the last try of a message it gives up after it (the wake after an
    /// outage): the session's person is told whatever the age of their keystroke (DRF-006).
    YuliusLastTry,
}

impl Sender {
    /// Whether the session's person is told that the message waits for their possible draft.
    fn tells_person(self, held: BoxHeld) -> bool {
        match self {
            Self::Person | Self::OwnerPastDraft => false,
            Self::Yulius => held.tells_person(),
            Self::YuliusLastTry => matches!(held, BoxHeld::Draft { .. }),
        }
    }
}

/// What the task says when a message of Yulius's own waits for a person's possible draft
/// (SPEC-480 DRF-006).
const DRAFT_HOLDS_PROBLEM: &str = "Yulius has a message for this session and does not type it: \
    someone typed in the terminal and sent no prompt since, so their text may still be in the \
    prompt box.";
/// The CLI can restore text itself, even after a prompt was submitted (SPEC-537).
const VISIBLE_DRAFT_PROBLEM: &str =
    "Yulius is holding a message because the session's prompt box contains text.";
const VISIBLE_DRAFT_NOTICE: &str = "Yulius is holding a message because the prompt box contains \
    text. Send the text, or clear it and send a prompt.";
/// What the person does about [`DRAFT_HOLDS_PROBLEM`].
const DRAFT_HOLDS_ACTION: &str = "Send what you typed in the terminal, or clear it and send \
    any prompt. Yulius types its message after the next prompt sent from there.";
/// The same, as one line in the session's terminal.
const DRAFT_HOLDS_NOTICE: &str = "Yulius holds a message for this session: text typed here was \
    not sent and may still be in the prompt box. Send it, or clear it and send any prompt, and \
    Yulius types its message.";

/// Only these exact stored reasons belong to the shared typing guard.
pub(crate) fn draft_flag(reason: &AttentionRecord) -> bool {
    [DRAFT_HOLDS_PROBLEM, VISIBLE_DRAFT_PROBLEM]
        .iter()
        .any(|problem| *reason == AttentionRecord::yulius(problem, DRAFT_HOLDS_ACTION))
}

fn shows_empty(harness: Harness, runtime: &TaskRuntime) -> bool {
    !runtime.screen_withheld() && runtime.hub.prompt_box_shown_empty(harness)
}

/// How long a line of Yulius's own waits for another typist to finish ([`Typing::wait`]).
pub(crate) const TYPING_TURN_WAIT: Duration = Duration::from_secs(5);

/// A run's "something is being typed" mark, held from the text to its Enter: one typist at a
/// time, whatever Yulius types (a message, `/compact`, a goal), so that no other text lands
/// between a line and its submit (SPEC-483 TYP-002). Cleared when the typing ends, even if the
/// call is dropped half-way.
pub(crate) struct Typing<'a>(&'a AtomicBool);

impl<'a> Typing<'a> {
    /// The mark, when nothing else is being typed.
    pub(crate) fn take(mark: &'a AtomicBool) -> Option<Self> {
        (!mark.swap(true, Ordering::SeqCst)).then_some(Self(mark))
    }

    /// The mark once the typing under way ended, at most `limit` later. `None` when it did
    /// not: a line of Yulius's own is then typed all the same (the other typist holds the
    /// mark for well under a second; a mark that stays set must not stop a recovery).
    pub(crate) async fn wait(mark: &'a AtomicBool, limit: Duration) -> Option<Self> {
        let asked = tokio::time::Instant::now();
        loop {
            if let Some(typing) = Self::take(mark) {
                return Some(typing);
            }
            if asked.elapsed() >= limit {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

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
        self.task_message_as(caller, args, None, None).await
    }

    /// [`App::tool_task_message`], an answer naming the request it answers (`asked_at_ms`, as
    /// the page's does, SPEC-512 ANS-004): a Telegram reply to a notice answers that notice's
    /// request, never a later one (TGI-002).
    pub(crate) async fn task_message_as(
        self: &Arc<Self>,
        caller: &ProjectCaller,
        args: &Value,
        asked_at_ms: Option<i64>,
        selected: Option<&TaskRecord>,
    ) -> AppResult<Value> {
        let ctx = self.manage_context(caller).await?;
        let task = self.agent_task_ref(caller, args).await?;
        if caller.task.as_ref().is_some_and(|own| own.id == task.id) {
            return Err(AppError::Forbidden("a task cannot message itself".into()));
        }
        // From someone else, while the task asks a question they may answer, the message is that
        // answer (SPEC-512 ANS-003): the page's answer, with its label, guards and audit.
        if !crate::tasks::owns(&task, &caller.user)
            && matches!(task.status, TaskStatus::Running | TaskStatus::Parked)
            && self.deps.store.task_needs_you(&task.id).await?.is_some()
        {
            let text = args
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| AppError::Invalid("missing string argument `text`".into()))?;
            self.answer_observed_question(&ctx, &task, text, asked_at_ms, selected)
                .await?;
            // Audited as a message too, with the channel it came through (TMS-005).
            let named = json!({
                "task": format!("#{} {}", task.number, task.title),
                "delivered": "answered",
            });
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
            let task = self
                .deps
                .store
                .task(&task.id)
                .await?
                .ok_or(AppError::NotFound)?;
            return Ok(json!({
                "task": self.task_brief(&task).await?,
                "delivered": "answered",
                "note": "The task asked a question you may answer: your message went to its \
                    session as your answer (a parked task resumed with it), and that question \
                    is closed; one its agent asked meanwhile stays open. The task stays its \
                    owner's.",
            }));
        }
        // A message steers the task, so it is its owner's (SPEC-490 OWN-004): queued into a
        // request, typed into a session, past a draft or not. That also keeps another person's
        // words from an agent holding the owner's personal secrets or MCP servers (SPEC-245,
        // SPEC-332).
        self.require_control(&ctx, &task).await?;
        if let Some(selected) = selected {
            task.check_selected_park(selected)?;
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
        // The owner decides that a message goes out together with what they left in their
        // task's prompt box (SPEC-487 TCT-006); every message is the owner's (above).
        let with_draft = match args.get("with_draft") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(with_draft)) => *with_draft,
            Some(_) => {
                return Err(AppError::Invalid(
                    "`with_draft` must be true or false".into(),
                ));
            }
        };
        // A prompt Yulius held for the person's decision (TSK-013) is back in the prompt box
        // and reads as a draft: it is decided on its card, never carried on by a message.
        if with_draft && self.deps.store.pending_hold(&task.id).await?.is_some() {
            return Err(AppError::Conflict(format!(
                "task #{} has a prompt that Yulius held for its person's decision, and it is \
                 back in the prompt box: they decide about it on the task's page first",
                task.number
            )));
        }
        // A session being parked takes nothing typed: the message would end with it (SPEC-512
        // PRK-003).
        if self.parking(&task.id) {
            return Err(AppError::Conflict(format!(
                "task #{} is parking its idle session: send the message again in a few \
                 seconds and it resumes the task",
                task.number
            )));
        }
        let delivered = if task.status == TaskStatus::Parked {
            // Writing to a parked task resumes it in its own session, with the message as its
            // first prompt; it is the person's answer, so the request it had open ends (SPEC-512
            // PRK-005, TMS-006).
            crate::race::reached(
                crate::race::Point::ParkedMessageSelected,
                std::path::Path::new(task.id.as_str()),
            );
            let login = self.login_of(&caller.user).await;
            let prompt = message_prompt(&login, caller.channel, &text);
            if !self.resume_parked(&task, Some(prompt)).await? {
                return Err(AppError::Conflict(format!(
                    "task #{} changed while it was resumed: try again",
                    task.number
                )));
            }
            self.end_needs_you(&task.id).await;
            "resumed"
        } else if task.status == TaskStatus::Running {
            let login = self.login_of(&caller.user).await;
            let from = if with_draft {
                Sender::OwnerPastDraft
            } else {
                Sender::Person
            };
            self.type_task_message(&task, &login, caller.channel, &text, from)
                .await?;
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
        let past_draft = with_draft && delivered == "sent";
        let mut named = json!({
            "task": format!("#{} {}", task.number, task.title),
            "delivered": delivered,
        });
        if past_draft {
            named["with_draft"] = json!(true);
        }
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
            "sent" if past_draft => {
                "Typed into the task's session behind whatever its person had left in the \
                 prompt box: both were submitted as one prompt, and the box is empty again."
            }
            "sent" => "Typed into the task's session: its agent reads it as soon as it can.",
            "resumed" => {
                "The task was parked: it resumes in its own session with your message as its \
                 first prompt (it waits in the queue while every session is in use)."
            }
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

    /// `POST /api/tasks/{tid}/needs-you/answer` (SPEC-516): `ctx`'s person, owner or not, answers
    /// what the running task's agent asked with `needs_you`: one labelled answer typed into its
    /// session, which ends the request. Control of the task does not move.
    ///
    /// # Errors
    /// `NotFound`/`Forbidden` without write access (a disabled account has none) or for a guest
    /// on another person's task; `Invalid` for an empty or oversized text; `Conflict` with
    /// nothing to answer, a task that is not running, an agent asking in its terminal, the
    /// owner's personal secrets or MCP servers within the run's reach, or a session that takes
    /// no typed text now.
    pub async fn answer_needs_you(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        text: &str,
        asked_at_ms: Option<i64>,
    ) -> AppResult<TaskSummary> {
        // A disabled account has no access to any project, so this refuses it too.
        let id = TaskId::parse(id).map_err(|_| AppError::NotFound)?;
        let task = self.deps.store.task(&id).await?.ok_or(AppError::NotFound)?;
        self.answer_observed_question(ctx, &task, text, asked_at_ms, None)
            .await
    }

    async fn answer_observed_question(
        self: &Arc<Self>,
        ctx: &AuthContext,
        observed: &TaskRecord,
        text: &str,
        asked_at_ms: Option<i64>,
        selected: Option<&TaskRecord>,
    ) -> AppResult<TaskSummary> {
        self.require_task(ctx, observed, TaskAction::Answer).await?;
        crate::race::reached(
            crate::race::Point::AnswerAuthorized,
            std::path::Path::new(observed.id.as_str()),
        );
        // The access check may have waited on GitHub: what the task does now is read after it
        // (review round 1), so a question opened in its terminal meanwhile is not answered.
        let task = self
            .deps
            .store
            .task(&observed.id)
            .await?
            .ok_or(AppError::NotFound)?;
        task.check_selected_park(observed)?;
        if let Some(selected) = selected {
            task.check_selected_park(selected)?;
        }
        let text = clean_message(text);
        if text.is_empty() {
            return Err(AppError::Invalid("the answer is empty".into()));
        }
        if text.len() > MAX_MESSAGE_BYTES {
            return Err(AppError::Invalid(format!(
                "the answer is longer than {} KiB",
                MAX_MESSAGE_BYTES / 1024
            )));
        }
        let owner = self.login_of(&task.owner).await;
        // A parked task's question stays open: the answer resumes it (SPEC-512 ANS-002).
        let parked = task.status == TaskStatus::Parked;
        if task.status != TaskStatus::Running && !parked {
            return Err(AppError::Conflict(format!(
                "task #{} is not running ({}), so its agent reads no answer now",
                task.number,
                task.status.as_str()
            )));
        }
        let Some(asked) = self.deps.store.task_needs_you(&task.id).await? else {
            return Err(AppError::Conflict(format!(
                "task #{} asks nobody anything now: there is nothing to answer",
                task.number
            )));
        };
        // The answer is to the request its person read (SPEC-512 ANS-004).
        if asked_at_ms.is_some_and(|at| at != asked.at) {
            return Err(AppError::Conflict(format!(
                "the question you answered on task #{} was replaced by a new one: read it and \
                 answer again",
                task.number
            )));
        }
        let mine = crate::tasks::owns(&task, &ctx.user.id);
        self.check_answer_authority(ctx, &task, &asked.named, &owner)
            .await?;
        let login = self.login_of(&ctx.user.id).await;
        if parked {
            // Resumed in its own session with the answer as its first prompt, as a message to a
            // parked task resumes it (PRK-005); it stays its owner's.
            let prompt = answer_prompt(&login, &owner, mine, &text);
            if !self.resume_parked(&task, Some(prompt)).await? {
                return Err(AppError::Conflict(format!(
                    "task #{} changed while it was resumed: try again",
                    task.number
                )));
            }
        } else {
            let paste = answer_paste(&login, &owner, mine, &text);
            self.type_into_session(&task, &paste, Typed::Prompt, Sender::Person)
                .await?;
        }
        crate::race::reached(
            crate::race::Point::AnswerTyped,
            std::path::Path::new(task.id.as_str()),
        );
        // The answer ends the request it answers, not one asked while it was typed (ANS-004).
        self.end_needs_you_asked_at(&task.id, asked.at, asked.cause.clone())
            .await;
        let detail = json!({
            "task": format!("#{} {}", task.number, task.title),
            "owner": owner,
            "by_owner": mine,
        });
        // The answer went out: a failed audit write is logged, not reported as a failed call
        // (which would invite sending it twice).
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &login,
                "task.needs_you.answered",
                Some(task.id.as_str()),
                Some(&detail.to_string()),
            )
            .await
        {
            tracing::warn!(task = %task.id, error = %err, "needs_you answer audit failed");
        }
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.task_summary(&task).await
    }

    /// Checks whether this person may answer the question the task is asking now.
    pub(crate) async fn check_answer_authority(
        &self,
        ctx: &AuthContext,
        task: &TaskRecord,
        named: &crate::needs_you::Named,
        owner: &str,
    ) -> AppResult<()> {
        let mine = crate::tasks::owns(task, &ctx.user.id);
        // A request that names people is theirs and its owner's (ANS-002).
        if !mine && named.narrows() {
            let login = self.login_of(&ctx.user.id).await.to_lowercase();
            if named.withdrawn.contains(&login) {
                return Err(AppError::Forbidden(format!(
                    "@{owner} withdrew you from task #{}'s question: only they{} answer it now",
                    task.number,
                    names(&named.who, " and ")
                )));
            }
            if !named.who.contains(&login) {
                return Err(AppError::Forbidden(format!(
                    "task #{} asks @{owner}{}, not you: its question is theirs to answer",
                    task.number,
                    names(&named.who, " and ")
                )));
            }
        }
        // A permission menu or a question in the terminal is answered with keys the paste would
        // press unseen (NYA-004). A live browser handoff also sets `activity = NeedsAttention` via
        // `AttentionKind::Yulius`, and has no terminal prompt blocking an inline answer (SPEC-578).
        let is_browser_attention = matches!(
            self.deps.store.stored_task_attention(&task.id).await,
            Ok(Some((attn, _)))
                if attn.kind == crate::attention::AttentionKind::Yulius
                    && attn
                        .link
                        .as_deref()
                        .and_then(crate::browser::browser_tab_from_link)
                        .is_some()
        );
        if task.activity == Some(Activity::NeedsAttention) && !is_browser_attention {
            return Err(AppError::Conflict(format!(
                "task #{}'s agent asks in its terminal now: @{owner} answers there, or take \
                 control of the task",
                task.number
            )));
        }
        // The owner's personal secrets and MCP servers act as them outside GitHub: another
        // person's words never drive them (as for `task_message`, SPEC-245, SPEC-332).
        if !mine && self.reaches_owners_personal(task).await? {
            return Err(AppError::Conflict(format!(
                "task #{} holds @{owner}'s personal secrets or MCP servers, so only @{owner} \
                 answers it; or take control of the task",
                task.number
            )));
        }
        Ok(())
    }

    /// Whether the owner's personal agent secrets (switched on for the task's project) or
    /// personal MCP servers reach the task's run (SPEC-245, SPEC-332): an automation's task holds
    /// none.
    pub(crate) async fn reaches_owners_personal(&self, task: &TaskRecord) -> AppResult<bool> {
        // An automation's task holds nobody's (`Holder::Nobody`): no read below finds any.
        let holder = Holder::of_task(task);
        let held = self.existing_runtime(&task.id).is_some_and(|runtime| {
            runtime
                .personal_mcp_of
                .lock()
                .map_or(true, |held| held.is_some())
        });
        if held || self.holds_personal_mcp(task).await? {
            return Ok(true);
        }
        Ok(self
            .deps
            .store
            .vault_agent_in(&task.sandbox_id)
            .await?
            .iter()
            .any(|secret| secret.owner_id.is_some() && holder.may_hold(secret)))
    }

    /// Types a cleaned message into the running task's session (TMS-003, TMS-004, TMS-006).
    async fn type_task_message(
        &self,
        task: &TaskRecord,
        login: &str,
        channel: Channel,
        text: &str,
        from: Sender,
    ) -> AppResult<()> {
        let paste = if from == Sender::OwnerPastDraft {
            message_paste_behind_draft(login, channel, text)
        } else {
            message_paste(login, channel, text)
        };
        self.type_into_session(task, &paste, Typed::Prompt, from)
            .await?;
        // The person spoke to the task (through their Superagent or their own agent): what it
        // asked them is answered.
        self.end_needs_you(&task.id).await;
        Ok(())
    }

    /// Types `paste` (one bracketed paste) and then Enter into the running task's session, once
    /// it takes typed text (TMS-004): its session started, no person's draft may sit in its
    /// prompt box and none typed in the last minute (SPEC-480), and nothing else is being
    /// typed. Also Yulius's own "Yulius is back; continue" (SPEC-358 MSB-007). `what` says
    /// whether Claude Code will submit it as a prompt, `from` who is told when a person's
    /// possible draft holds it back.
    pub(crate) async fn type_into_session(
        &self,
        task: &TaskRecord,
        paste: &[u8],
        what: Typed<'_>,
        from: Sender,
    ) -> AppResult<()> {
        self.type_into_session_if(task, paste, what, from, None::<std::future::Ready<bool>>)
            .await
    }

    /// A machine notification verifies its recipient and decision after draft lookup. It defers if the draft lock
    /// is busy, so neither a hosted read under the lock nor a later wait can stale that decision.
    pub(crate) async fn type_into_session_if(
        &self,
        task: &TaskRecord,
        paste: &[u8],
        what: Typed<'_>,
        from: Sender,
        admit: Option<impl std::future::Future<Output = bool>>,
    ) -> AppResult<()> {
        let runtime = self.existing_runtime(&task.id).ok_or_else(|| {
            AppError::Conflict(format!("task #{} has no live session", task.number))
        })?;
        // Its session is ending for a park (SPEC-512 PRK-003): what is typed now is lost.
        if self.parking(&task.id) {
            return Err(AppError::Conflict(format!(
                "task #{} is parking its idle session: try again in a few seconds",
                task.number
            )));
        }
        if runtime.typing_opening.load(Ordering::SeqCst) {
            return Err(AppError::Conflict(format!(
                "Yulius is still typing task #{}'s first message: try again in a minute",
                task.number
            )));
        }
        // A keystroke that may have left a draft can be released by the exact empty-box
        // picture. Resolve the recovery mark before judging the screen and key together.
        let attempt = if from != Sender::OwnerPastDraft
            && self
                .box_held(&task.id)
                .is_some_and(BoxHeld::ends_at_an_empty_box)
            && shows_empty(task.harness, &runtime)
        {
            let attempt = self.deps.store.live_attempt(&task.id).await?;
            crate::race::reached(
                crate::race::Point::BoxSeenEmpty,
                Path::new(task.id.as_str()),
            );
            attempt
        } else {
            None
        };
        // For a screen-based release, serialize the final decision, paste, Enter and mark
        // cleanup with viewer keys and submit hooks (SPEC-537). A key arriving during
        // the lookup above participates in this decision.
        let guarded = admit.is_some();
        if let Some(admit) = admit
            && !admit.await
        {
            return Err(AppError::Conflict(
                "the notification is no longer due".into(),
            ));
        }
        let mut mark = if attempt.is_some() && guarded {
            Some(runtime.draft_mark.try_lock().map_err(|_| {
                AppError::Conflict("the prompt box changed during notification verification".into())
            })?)
        } else if attempt.is_some() {
            Some(runtime.draft_mark.lock().await)
        } else {
            None
        };
        // A screen that is withheld, or misses output dropped while it was (agent secrets could
        // not be read, ADR-0317), is not what the program drew: a dialog may show there that
        // nobody saw, and a paste and an Enter would answer it.
        if runtime.screen_withheld() {
            // Its person learns it here, once: until the task is stopped and continued, every
            // message for it is kept out (review of SPEC-483 slice 5, round 4).
            self.screen_withheld(&task.id).await;
            return Err(AppError::Conflict(format!(
                "{} into task #{}: its person stops and continues the task (and first replaces \
                 or deletes the agent secret that cannot be read, while the terminal says so)",
                crate::supervisor::SCREEN_WITHHELD,
                task.number
            )));
        }
        // Before Claude Code's session starts, typed input is lost or left unsubmitted, and a
        // trust dialog only a person answers (SPEC-117 ONE-005) may show or be about to: a paste
        // and an Enter would answer it with its default. Without a SessionStart (a CLI without
        // the hook; a reattached run's is read from its marker), the run is taken as started
        // half the goal-ready timeout after its spawn, as the goal typist does
        // (`wait_input_ready`).
        if !runtime.session_started.load(Ordering::SeqCst) {
            if trust_prompt_shown(&runtime.hub.screen_text()) {
                return Err(AppError::Conflict(format!(
                    "task #{} waits for a person to answer Claude Code's folder-trust question \
                     in its terminal; tell the person, then try again",
                    task.number
                )));
            }
            let ready_ms = i64::try_from(self.config.goal_ready_ms / 2).unwrap_or(i64::MAX);
            let spawned = runtime.spawned_ms.load(Ordering::SeqCst);
            if self.now().0.saturating_sub(spawned) < ready_ms {
                return Err(AppError::Conflict(format!(
                    "task #{} is still starting its session: try again in a minute",
                    task.number
                )));
            }
        }
        let typed = runtime.typed_at_ms.load(Ordering::SeqCst);
        let emptied = runtime.empty_time_for_typing();
        let held = yulius_domain::intake::box_held(self.now().0, typed, emptied);
        let released = attempt.filter(|_| {
            held.is_some_and(BoxHeld::ends_at_an_empty_box) && shows_empty(task.harness, &runtime)
        });
        // The recovered file dates the first key in a draft. A later key (including
        // the one that cleared it) could have arrived immediately before reattachment.
        if released.is_some()
            && mark
                .as_ref()
                .and_then(|mark| mark.recovered_at)
                .is_some_and(|at| {
                    self.now().0.saturating_sub(at) <= yulius_domain::intake::TYPED_WINDOW_MS
                })
        {
            return Err(AppError::Conflict(format!(
                "task #{}'s recovered prompt box may have received a key in the last minute: try again in a minute",
                task.number
            )));
        }
        match held {
            Some(BoxHeld::Draft { .. }) if released.is_some() => {}
            // Its owner sends past their own text, once they are no longer typing it: it
            // goes out with the message.
            Some(BoxHeld::Draft { typed_ms_ago })
                if from == Sender::OwnerPastDraft
                    && typed_ms_ago > yulius_domain::intake::TYPED_WINDOW_MS => {}
            Some(BoxHeld::Draft { .. }) if from == Sender::OwnerPastDraft => {
                return Err(AppError::Conflict(format!(
                    "a person typed into task #{}'s terminal in the last minute: try again in \
                     a minute, or tell the person",
                    task.number
                )));
            }
            Some(held @ BoxHeld::Draft { .. }) => {
                drop(mark);
                if from.tells_person(held) {
                    self.tell_of_held_message(task, typed, DRAFT_HOLDS_PROBLEM, DRAFT_HOLDS_NOTICE)
                        .await;
                }
                return Err(AppError::Conflict(format!(
                    "a person typed into task #{}'s terminal and sent no prompt since, so \
                     their text may still be in its prompt box and nothing is typed behind it: \
                     tell the person to send it, or to clear it and send any prompt, then try \
                     again (when the task's owner says to send past it: task_message with \
                     with_draft)",
                    task.number
                )));
            }
            Some(BoxHeld::AtKeyboard) => {
                return Err(AppError::Conflict(format!(
                    "a person typed into task #{}'s terminal in the last minute: try again in \
                     a minute, or tell the person",
                    task.number
                )));
            }
            None => {}
        }
        let Some(_typing) = Typing::take(&runtime.typing_message) else {
            return Err(AppError::Conflict(format!(
                "another message to task #{} is being typed: try again in a moment",
                task.number
            )));
        };
        if task.harness == Harness::ClaudeCode
            && from != Sender::OwnerPastDraft
            && runtime.hub.shows_draft()
        {
            drop(mark);
            if matches!(from, Sender::Yulius | Sender::YuliusLastTry) {
                self.tell_of_held_message(
                    task,
                    emptied.max(1),
                    VISIBLE_DRAFT_PROBLEM,
                    VISIBLE_DRAFT_NOTICE,
                )
                .await;
            }
            return Err(AppError::Conflict(format!(
                "task #{}'s prompt box contains text. Ask its person to send or clear that text, then try again.",
                task.number
            )));
        }
        // From here a message is in the session's prompt box and not submitted, and a restart
        // would lose it (SPEC-468 CCF-011). Stamped before it is typed: its submit comes after
        // (Claude Code runs the prompt hook some 30 ms after the Enter below, also while a
        // turn runs). A command submits nothing, and a text that could not be typed is nowhere.
        let before = runtime.message_typed_ms.load(Ordering::SeqCst);
        if matches!(what, Typed::Prompt | Typed::Notice(_)) {
            runtime
                .message_typed_ms
                .store(self.now().0, Ordering::SeqCst);
        }
        let pasted = match what {
            Typed::Notice(attempt) => {
                runtime
                    .write_notice(attempt, self.now(), SessionInput::Paste(paste))
                    .await
            }
            Typed::Prompt | Typed::Command => {
                self.send_session_input(&task.id, SessionInput::Paste(paste))
                    .await
            }
        };
        if let Err(err) = pasted {
            runtime.message_typed_ms.store(before, Ordering::SeqCst);
            return Err(err);
        }
        if matches!(what, Typed::Notice(_)) {
            crate::race::reached(
                crate::race::Point::PrNoticePasted,
                Path::new(task.id.as_str()),
            );
        }
        tokio::time::sleep(SUBMIT_DELAY).await;
        if task.mode == crate::model::TaskMode::Goal
            && matches!(from, Sender::Yulius | Sender::YuliusLastTry)
            && let Some(text) = paste
                .strip_prefix(b"\x1b[200~")
                .and_then(|data| data.strip_suffix(b"\x1b[201~"))
        {
            self.note_issued_prompt(&task.id, text);
        }
        // The submit below carries the owner's draft to the agent with the message: the
        // prompt hook reads this to know the draft ended (also one known from its mark alone,
        // after a restart of the server). Set before the Enter: the hook runs some 30 ms
        // after it.
        let past_draft = from == Sender::OwnerPastDraft && what == Typed::Prompt;
        if past_draft {
            runtime.sent_past_draft.store(true, Ordering::SeqCst);
        }
        // The text is in the prompt box but not sent: said, so a retry does not send it twice.
        let submitted = match what {
            Typed::Notice(attempt) => {
                runtime
                    .write_notice(attempt, self.now(), SessionInput::Submit)
                    .await
            }
            Typed::Prompt | Typed::Command => {
                self.send_session_input(&task.id, SessionInput::Submit)
                    .await
            }
        };
        submitted.map_err(|err| {
            if past_draft {
                runtime.sent_past_draft.store(false, Ordering::SeqCst);
            }
            AppError::Conflict(format!(
                "the message was pasted into task #{}'s session but not submitted ({}): it may \
                 still be in its prompt box, so look before sending it again",
                task.number,
                err.public_message()
            ))
        })?;
        // The task's owner can send the new instruction through a tool as well as by
        // typing in its terminal. A failed send, a command and a machine wake spend
        // none of the goal's restored budget.
        if task.mode == crate::model::TaskMode::Goal
            && what == Typed::Prompt
            && matches!(from, Sender::Person | Sender::OwnerPastDraft)
        {
            let update = crate::ports::store::TaskFieldUpdate {
                gate_blocks: Some(0),
                ..Default::default()
            };
            if let Err(err) = self
                .deps
                .store
                .update_task(&task.id, &update, self.now())
                .await
            {
                tracing::warn!(task = %task.id, error = %err, "could not reset the goal attempts after an owner message");
            }
        }
        if let (Some(attempt), Some(mark)) = (&released, &mut mark) {
            runtime.box_emptied_ms.fetch_max(typed, Ordering::SeqCst);
            runtime
                .emptied_seq
                .fetch_max(runtime.typed_seq.load(Ordering::SeqCst), Ordering::SeqCst);
            mark.written = false;
            crate::supervisor::clear_person_typed(Path::new(&attempt.run_dir));
        }
        drop(mark);
        if released.is_some() {
            self.settle_draft_flag(&task.id).await;
        }
        Ok(())
    }

    /// Flags a waiting automatic message, with one terminal notice per last key or, for a
    /// visible restore, per last submit. A later pass restores a flag that the turn settled.
    async fn tell_of_held_message(
        &self,
        task: &TaskRecord,
        stamp: i64,
        problem: &str,
        notice: &str,
    ) {
        let reason = AttentionRecord::yulius(problem, DRAFT_HOLDS_ACTION);
        if let Err(err) = self.flag_attention(&task.id, reason).await {
            // Not told: the next pass tries again.
            tracing::warn!(task = %task.id, error = %err, "could not flag the task whose draft holds a message");
            return;
        }
        let runtime = self.runtime(&task.id);
        if runtime.draft_told_ms.swap(stamp, Ordering::SeqCst) != stamp {
            runtime.hub.notice(notice.to_owned());
            tracing::info!(task = %task.id, "a message waits for a person's draft: the task is flagged");
        }
    }

    /// PR news may retry the exact flag of a waiting message once the person has
    /// left an empty conversation box. Delivery itself judges the key and screen again.
    pub(crate) async fn held_message_flag_outlived(&self, task: &TaskRecord) -> bool {
        self.held_draft_box_cleared(task)
            && matches!(self.deps.store.task_attention(&task.id).await, Ok(Some((reason, _))) if draft_flag(&reason))
    }

    pub(crate) fn held_draft_box_cleared(&self, task: &TaskRecord) -> bool {
        self.typed_recently(&task.id).is_none()
            && self
                .existing_runtime(&task.id)
                .is_some_and(|runtime| shows_empty(task.harness, &runtime))
    }

    /// Ends the flag that says a message of Yulius's own waits for a person's draft
    /// ([`Self::tell_of_held_message`]) once no draft holds the prompt box (SPEC-480 DRF-007).
    /// A prompt that starts a turn sets the task Working, which ends every flag; one that starts
    /// none (`/compact`, a prompt answered after its turn ended) leaves the task as it was, so
    /// the flag would outlast its reason and keep the pull-request watch quiet (a flagged task
    /// gets nothing typed). Only this flag, and only while the task still reads it.
    pub(crate) async fn settle_draft_flag(&self, task_id: &TaskId) {
        let runtime = self.runtime(task_id);
        let held = runtime.attention_lock.lock().await;
        let store = &self.deps.store;
        let flagged = match store.task_attention(task_id).await {
            Ok(Some((reason, flagged))) if draft_flag(&reason) => flagged,
            Ok(_) => return,
            Err(err) => {
                tracing::warn!(task = %task_id, error = %err, "reading a flag's reason failed");
                return;
            }
        };
        let next = if runtime.turn_in_progress() {
            Activity::Working
        } else {
            Activity::after_clean_turn(runtime.goal_achieved.load(Ordering::SeqCst))
        };
        let from = (Some(Activity::NeedsAttention), flagged);
        match store
            .set_activity_from(task_id, from, Some(next), self.now())
            .await
        {
            Ok(true) => {
                drop(held);
                tracing::info!(task = %task_id, "the prompt box was emptied: the flag of a held message ends");
                self.publish_task(task_id).await;
            }
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(task = %task_id, error = %err, "could not end the flag of a held message");
            }
        }
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
    use super::{answer_paste, clean_message, message_paste};
    use crate::coordination::Channel;

    #[test]
    fn only_claude_sessions_use_the_empty_conversation_frame() {
        let rt = super::TaskRuntime::new();
        let rule = "─".repeat(20);
        let frame = format!("\x1b[?1049h\x1b[2J\x1b[2;1H{rule}\x1b[3;1H❯ \x1b[4;1H{rule}\x1b[3;3H");
        rt.hub.resize(20, 6);
        rt.hub.output(1, frame.as_bytes());
        assert!(super::shows_empty(super::Harness::ClaudeCode, &rt));
        assert!(!super::shows_empty(super::Harness::Codex, &rt));
        assert!(!super::shows_empty(super::Harness::Antigravity, &rt));
    }

    #[test]
    fn only_the_focused_codex_placeholder_proves_its_composer_empty() {
        let rt = super::TaskRuntime::new();
        let frame = concat!(
            "\x1b[?1049h\x1b[2J",
            "\x1b[18;1H\x1b[1m›\x1b[22m \x1b[2mAsk Codex to do anything\x1b[22m",
            "\x1b[20;1H  GPT-6.2-Sol default · project",
            "\x1b[40;1H  \x1b[1m←\x1b[22m for agents · \x1b[1m?\x1b[22m for shortcuts",
            "\x1b[18;3H\x1b[?25h",
        );
        for (edit, empty) in [
            ("", true),
            ("\x1b[18;1H\x1b[1m»\x1b[0m\x1b[18;3H", true),
            (
                "\x1b[40;1H\x1b[2K  \x1b[1m?\x1b[0m for shortcuts\x1b[18;3H",
                true,
            ),
            // History can restore a remote image independently of the editable text.
            ("\x1b[16;3H[Image #1]\x1b[40;1H\x1b[2K\x1b[18;3H", false),
            // A partial repaint can leave the previous empty footer beneath that label.
            ("\x1b[16;3H[Image #2]\x1b[18;3H", false),
            (
                "\x1b[40;1H\x1b[2K  \x1b[1m←\x1b[0m for agentsXYZ\x1b[18;3H",
                false,
            ),
            // When attachment rows are clipped, the empty-only footer hint is still absent.
            ("\x1b[40;1H\x1b[2K\x1b[18;3H", false),
            ("\x1b[40;3H←\x1b[18;3H", false),
            ("\x1b[?25l", false),
            ("\x1b[?1049l", false),
            ("\x1b[18;4H", false),
            ("\x1b[17;3H", false),
            ("\x1b[18;1H›\x1b[18;3H", false),
            ("\x1b[18;1H\x1b[1;2m›\x1b[0m\x1b[18;3H", false),
            ("\x1b[18;2Hx\x1b[18;3H", false),
            ("A\x1b[18;3H", false),
            ("\x1b[18;26Hg\x1b[18;3H", false),
            ("Ask Codex to do anything\x1b[18;3H", false),
            ("\x1b[2mTry another command\x1b[0m\x1b[K\x1b[18;3H", false),
            ("\x1b[K[Image #1]\x1b[18;3H", false),
            ("\x1b[18;30Hmore text\x1b[18;3H", false),
            ("\x1b[19;1Hcontinued draft\x1b[18;3H", false),
        ] {
            rt.hub.output(1, frame.as_bytes());
            rt.hub.output(2, edit.as_bytes());
            assert_eq!(
                super::shows_empty(super::Harness::Codex, &rt),
                empty,
                "{edit:?}"
            );
            assert!(!super::shows_empty(super::Harness::ClaudeCode, &rt));
            assert!(!super::shows_empty(super::Harness::Antigravity, &rt));
        }
        // The same cells in the primary buffer can be transcript output.
        rt.hub.output(3, b"\x1b[?1049l");
        rt.hub
            .output(4, frame.trim_start_matches("\x1b[?1049h").as_bytes());
        assert!(!super::shows_empty(super::Harness::Codex, &rt));
    }

    #[test]
    fn only_the_two_stored_draft_reasons_allow_a_pr_notice_to_retry() {
        for problem in [super::DRAFT_HOLDS_PROBLEM, super::VISIBLE_DRAFT_PROBLEM] {
            let held = super::AttentionRecord::yulius(problem, super::DRAFT_HOLDS_ACTION);
            assert!(super::draft_flag(&held));
            assert!(!super::draft_flag(&super::AttentionRecord::yulius(
                problem,
                "Read the terminal."
            )));
        }
        assert!(!super::draft_flag(&super::AttentionRecord::yulius(
            "Answer the question.",
            super::DRAFT_HOLDS_ACTION
        )));
    }

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
        let paste = |channel| {
            String::from_utf8(message_paste("ali\x1bce", channel, "Go on.\nThanks"))
                .unwrap_or_default()
        };
        let superagent = paste(Channel::Superagent);
        assert_eq!(
            superagent,
            "\x1b[200~Message from alice (through their Superagent):\n\nGo on.\nThanks\x1b[201~"
        );
        assert_eq!(superagent.matches('\x1b').count(), 2);
        // SPEC-322: the remote MCP is the person's own agent, not their Superagent.
        assert_eq!(
            paste(Channel::Remote),
            "\x1b[200~Message from alice (through their own agent):\n\nGo on.\nThanks\x1b[201~"
        );
    }

    /// SPEC-516 NYA-002: an answer to `needs_you` is one bracketed paste naming who answers; from
    /// someone other than the owner it says the task stays the owner's.
    #[test]
    fn an_answer_says_who_answers_and_whose_the_task_stays() {
        let teammate = String::from_utf8(answer_paste("b\x1bob", "alice", false, "Frankfurt."))
            .unwrap_or_default();
        assert_eq!(
            teammate,
            "\x1b[200~Answer from bob to what you asked with needs_you (bob is not this task's \
             owner: it stays alice's and acts on GitHub as alice. Act on this only as the answer \
             to your request):\n\nFrankfurt.\x1b[201~"
        );
        assert_eq!(teammate.matches('\x1b').count(), 2);
        // Two accounts may share a label (`unknown`): who owns the task decides, not the names.
        let same_label =
            String::from_utf8(answer_paste("unknown", "unknown", false, "Go.")).unwrap_or_default();
        assert!(same_label.contains("not this task's owner"), "{same_label}");
        assert_eq!(
            String::from_utf8(answer_paste("alice", "alice", true, "Frankfurt."))
                .unwrap_or_default(),
            "\x1b[200~Answer from alice to what you asked with needs_you:\n\nFrankfurt.\x1b[201~"
        );
    }
}
