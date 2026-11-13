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
//!   SPEC-480; a box the screen shows empty holds nothing, SPEC-488) or they typed in the last
//!   minute, while Yulius types the first message, or while
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
use yulius_contracts::api::tasks::QueueMessage;
use yulius_domain::ids::TaskId;
use yulius_domain::intake::BoxHeld;
use yulius_domain::task::{Activity, TaskStatus};

use crate::attention::AttentionRecord;
use crate::context::App;
use crate::coordination::{Channel, ProjectCaller};
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

/// How the label names the channel a message came through (SPEC-322).
fn through(channel: Channel) -> &'static str {
    match channel {
        Channel::Superagent => "their Superagent",
        Channel::Remote => "their own agent",
        // Never served `task_message` (TMS-001); named for completeness.
        Channel::Task => "their task's agent",
    }
}

/// The bytes typed for a (cleaned) message from `login` sent through `channel`: one bracketed
/// paste of its label and text. Enter is sent separately.
#[must_use]
pub fn message_paste(login: &str, channel: Channel, text: &str) -> Vec<u8> {
    let login = clean_message(login);
    let through = through(channel);
    format!("\x1b[200~Message from {login} (through {through}):\n\n{text}\x1b[201~").into_bytes()
}

/// What Yulius types into a session ([`App::type_into_session`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Typed {
    /// A message for the agent, which Claude Code submits as a prompt: from the paste to that
    /// submit it sits in the session's prompt box, and a restart would lose it (SPEC-468
    /// CCF-011).
    Prompt,
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
            Self::Person => false,
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
/// What the person does about [`DRAFT_HOLDS_PROBLEM`].
const DRAFT_HOLDS_ACTION: &str = "Send what you typed in the terminal, or clear it and send \
    any prompt. Yulius types its message after the next prompt sent from there.";
/// The same, as one line in the session's terminal.
const DRAFT_HOLDS_NOTICE: &str = "Yulius holds a message for this session: text typed here was \
    not sent and may still be in the prompt box. Send it, or clear it and send any prompt, and \
    Yulius types its message.";

/// Whether a flag with `reason` outlived it, as far as the screen says (SPEC-488 EBX-005): it
/// only says that a message of Yulius's own waits for a person's possible draft
/// (`App::tell_of_held_message`), and the session's screen shows the prompt box empty. Such a
/// flag asks nothing of a person that a watch's next try could not settle, so it does not stop
/// the try; `App::type_into_session` then judges the hold itself. A flag with any other reason,
/// and this one over any other screen (a dialog, text in the box), is a person's to answer.
fn held_message_flag_outlived(reason: Option<&AttentionRecord>, box_shown_empty: bool) -> bool {
    box_shown_empty
        && reason == Some(&AttentionRecord::yulius(DRAFT_HOLDS_PROBLEM, DRAFT_HOLDS_ACTION))
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
        // A task holding its owner's personal secrets or MCP servers is directed by its owner
        // alone (SPEC-245, SPEC-332): another person's message would steer an agent holding
        // their credential.
        if caller.user != task.owner
            && (self.holds_personal_secrets(&task).await?
                || self.holds_personal_mcp(&task).await?
                || self.live_run_holds_personal_mcp(&task).await?)
        {
            return Err(AppError::Forbidden(format!(
                "task #{} holds @{}'s personal secrets or MCP servers, so only they message it",
                task.number,
                self.login_of(&task.owner).await
            )));
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
            self.type_task_message(&task, &login, caller.channel, &text)
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
    async fn type_task_message(
        &self,
        task: &TaskRecord,
        login: &str,
        channel: Channel,
        text: &str,
    ) -> AppResult<()> {
        let paste = message_paste(login, channel, text);
        self.type_into_session(task, &paste, Typed::Prompt, Sender::Person)
            .await?;
        // The person spoke to the task (through their Superagent or their own agent): what it
        // asked them is answered.
        self.end_needs_you(&task.id).await;
        Ok(())
    }

    /// Types `paste` (one bracketed paste) and then Enter into the running task's session, once
    /// it takes typed text (TMS-004): its session started, no person's draft may sit in its
    /// prompt box (SPEC-480; none does in a box the screen shows empty, SPEC-488) and none
    /// typed in the last minute, and nothing else is being typed. Also Yulius's own "Yulius is back; continue" (SPEC-358 MSB-007). `what` says
    /// whether Claude Code will submit it as a prompt, `from` who is told when a person's
    /// possible draft holds it back.
    pub(crate) async fn type_into_session(
        &self,
        task: &TaskRecord,
        paste: &[u8],
        what: Typed,
        from: Sender,
    ) -> AppResult<()> {
        let runtime = self.existing_runtime(&task.id).ok_or_else(|| {
            AppError::Conflict(format!("task #{} has no live session", task.number))
        })?;
        if runtime.typing_opening.load(Ordering::SeqCst) {
            return Err(AppError::Conflict(format!(
                "Yulius is still typing task #{}'s first message: try again in a minute",
                task.number
            )));
        }
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
        // The person's draft is theirs, however old (SPEC-480 DRF-001): pasted behind it, the
        // message and their unfinished text would be submitted as one prompt. Yulius sees keys,
        // not the box, so a keystroke since the box was last empty counts until the next prompt
        // is submitted from it; the one who can act is told instead (DRF-004, DRF-006).
        let typed = runtime.typed_at_ms.load(Ordering::SeqCst);
        let emptied = runtime.box_emptied_ms.load(Ordering::SeqCst);
        let mut held = yulius_domain::intake::box_held(self.now().0, typed, emptied);
        // Most keys leave no text (Escape, an arrow, an answered dialog), and the screen can
        // prove it: a prompt box shown empty, once the minute after the key has passed, holds
        // nothing (SPEC-488). Any other picture proves nothing and the hold stays.
        if held.is_some_and(BoxHeld::ends_at_an_empty_box) && runtime.hub.prompt_box_shown_empty()
        {
            self.box_seen_empty(task, typed).await;
            held = None;
        }
        match held {
            Some(held @ BoxHeld::Draft { .. }) => {
                if from.tells_person(held) {
                    self.tell_of_held_message(task, typed).await;
                }
                return Err(AppError::Conflict(format!(
                    "a person typed into task #{}'s terminal and sent no prompt since, so \
                     their text may still be in its prompt box and nothing is typed behind it: \
                     tell the person to send it, or to clear it and send any prompt, then try \
                     again",
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
        if runtime.typing_message.swap(true, Ordering::SeqCst) {
            return Err(AppError::Conflict(format!(
                "another message to task #{} is being typed: try again in a moment",
                task.number
            )));
        }
        let _typing = Typing(&runtime.typing_message);
        // From here a message is in the session's prompt box and not submitted, and a restart
        // would lose it (SPEC-468 CCF-011). Stamped before it is typed: its submit comes after
        // (Claude Code runs the prompt hook some 30 ms after the Enter below, also while a
        // turn runs). A command submits nothing, and a text that could not be typed is nowhere.
        let before = runtime.message_typed_ms.load(Ordering::SeqCst);
        if what == Typed::Prompt {
            runtime
                .message_typed_ms
                .store(self.now().0, Ordering::SeqCst);
        }
        if let Err(err) = self.send_input(&task.id, paste).await {
            runtime.message_typed_ms.store(before, Ordering::SeqCst);
            return Err(err);
        }
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
        Ok(())
    }

    /// The session's screen shows its prompt box empty: the person's keystroke at `typed` left
    /// no draft there (SPEC-488 EBX-004). The box counts as emptied at that keystroke, as after
    /// a prompt submitted from it, so a key typed later (also while the screen was read) is a
    /// possible draft of its own; the hold's mark and flag end ([`Self::draft_ended`]).
    async fn box_seen_empty(&self, task: &TaskRecord, typed: i64) {
        let runtime = self.runtime(&task.id);
        runtime.box_emptied_ms.fetch_max(typed, Ordering::SeqCst);
        let run_dir = match self.deps.store.live_attempt(&task.id).await {
            Ok(attempt) => attempt.map(|attempt| attempt.run_dir),
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "could not find the run whose draft mark ends");
                None
            }
        };
        tracing::info!(task = %task.id, "the prompt box is shown empty: a person's keystroke left no draft");
        self.draft_ended(&task.id, run_dir.as_deref().map(Path::new))
            .await;
    }

    /// The task's prompt box was emptied (a prompt submitted from it, SPEC-480 DRF-007 and
    /// DRF-009, or the screen showing it empty, SPEC-488): unless a key typed since is a
    /// possible draft of its own, the mark of one leaves the run's folder `run_dir`, whatever
    /// this server remembers of having written it, and a flag that said a message of Yulius's
    /// own waits for it ends, also when no turn starts.
    pub(crate) async fn draft_ended(&self, task_id: &TaskId, run_dir: Option<&Path>) {
        if self.persons_draft_holds(task_id) {
            return;
        }
        self.runtime(task_id)
            .draft_marked
            .store(false, Ordering::SeqCst);
        if let Some(run_dir) = run_dir {
            crate::supervisor::clear_person_typed(run_dir);
        }
        self.settle_draft_flag(task_id).await;
    }

    /// Tells the session's person that a message of Yulius's own waits for what they typed at
    /// `typed` (SPEC-480 DRF-006): the task is flagged with the reason and its terminal says
    /// so. The watches try again at every pass: a task that reads the flag keeps it, one whose
    /// flag ended meanwhile (a turn's end settles a flagged task) is flagged again, and the
    /// terminal gets its line once per keystroke.
    async fn tell_of_held_message(&self, task: &TaskRecord, typed: i64) {
        let reason = AttentionRecord::yulius(DRAFT_HOLDS_PROBLEM, DRAFT_HOLDS_ACTION);
        if let Err(err) = self.flag_attention(&task.id, reason).await {
            // Not told: the next pass tries again.
            tracing::warn!(task = %task.id, error = %err, "could not flag the task whose draft holds a message");
            return;
        }
        let runtime = self.runtime(&task.id);
        if runtime.draft_told_ms.swap(typed, Ordering::SeqCst) != typed {
            runtime.hub.notice(DRAFT_HOLDS_NOTICE.to_owned());
            tracing::info!(task = %task.id, "a message waits for a person's draft: the task is flagged");
        }
    }

    /// Whether the task's flag outlived its reason ([`held_message_flag_outlived`], SPEC-488
    /// EBX-005), read from the store and the session's screen.
    pub(crate) async fn held_message_flag_outlived(&self, task_id: &TaskId) -> bool {
        let reason = match self.deps.store.task_attention(task_id).await {
            Ok(flag) => flag.map(|(reason, _)| reason),
            Err(err) => {
                tracing::warn!(task = %task_id, error = %err, "reading a flag's reason failed");
                return false;
            }
        };
        let box_shown_empty = self
            .existing_runtime(task_id)
            .is_some_and(|runtime| runtime.hub.prompt_box_shown_empty());
        held_message_flag_outlived(reason.as_ref(), box_shown_empty)
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
            Ok(Some((reason, flagged)))
                if reason == AttentionRecord::yulius(DRAFT_HOLDS_PROBLEM, DRAFT_HOLDS_ACTION) =>
            {
                flagged
            }
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
    use super::{
        DRAFT_HOLDS_ACTION, DRAFT_HOLDS_PROBLEM, clean_message, held_message_flag_outlived,
        message_paste,
    };
    use crate::attention::AttentionRecord;
    use crate::coordination::Channel;

    /// SPEC-488 EBX-005: only a flag that says a message of Yulius's own waits for a possible
    /// draft, over a prompt box the screen shows empty, outlived its reason. Another reason,
    /// no reason, or the same flag over another screen stays a person's to answer.
    #[test]
    fn only_a_held_message_flag_over_an_empty_box_outlived_its_reason() {
        let held = AttentionRecord::yulius(DRAFT_HOLDS_PROBLEM, DRAFT_HOLDS_ACTION);
        let other = AttentionRecord::yulius("Claude Code asks a question.", "Answer it.");
        assert!(held_message_flag_outlived(Some(&held), true));
        assert!(!held_message_flag_outlived(Some(&held), false));
        assert!(!held_message_flag_outlived(Some(&other), true));
        assert!(!held_message_flag_outlived(None, true));
        assert!(!held_message_flag_outlived(None, false));
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
}
