//! What a task asks of a person, read and answered for them (SPEC-487 slice 4, QST-001 to
//! QST-005; wall 7): why a task waits for a person, and two decisions for the person: deciding
//! a prompt Yulius held (the card on the task's page) and dismissing the task's request to its
//! person (`needs_you`, its agent's or one Yulius opened for it), which no page did: it ended
//! only with a message to the task or by its agent.
//!
//! Contract:
//! - what a task says about itself (`App::waits_why`) is what its page shows: the list's
//!   group, the task's request (`needs_you`), the reason of its flag, a held prompt, a pause.
//!   It is read by everyone who reads the task list, a task's agent included (without a held
//!   prompt's words); the texts are an agent's, Claude Code's or a person's own words, each
//!   shown as one bounded line without what hides or reverses text;
//! - the decisions are the task owner's: a person's function with their [`AuthContext`],
//!   write access checked at the call;
//! - a request is ended only while it still asks what was read: one asked meanwhile stays.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{Value, json};
use yulius_contracts::api::intake::{HeldPromptOutcome, ResolveHeldPrompt};
use yulius_domain::permission::Action;
use yulius_domain::task::TaskGroup;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::TaskRecord;

/// The most characters of a held prompt a task's brief carries.
const HELD_EXCERPT_CHARS: usize = 300;

/// The most characters of each text of a task's request a brief carries (they are stored
/// within 2 000).
const SHOWN_TEXT_CHARS: usize = 2_000;

/// The decisions on a held prompt an agent makes for its person (the page has two more:
/// `skip_after`, which names steps, and `revert`, which discards the task's work).
pub const HELD_PROMPT_ACTIONS: [&str; 4] = ["send", "dismiss", "parallel", "after"];

/// The list's group of a task, as tools name it.
#[must_use]
pub fn group_name(group: TaskGroup) -> &'static str {
    match group {
        TaskGroup::NeedsYou => "needs_you",
        TaskGroup::Working => "working",
        TaskGroup::Queued => "queued",
        TaskGroup::Done => "done",
        TaskGroup::Ended => "ended",
    }
}

impl App {
    /// The group the task list shows `task` in, among the sandbox's pending requests
    /// `requested` ([`App::browser_requests`]).
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn shown_group(
        &self,
        task: &TaskRecord,
        requested: &HashSet<String>,
    ) -> AppResult<TaskGroup> {
        self.task_group(task, requested).await
    }

    /// Why `task` waits for a person, or is held, as its page shows it (QST-001): the fields a
    /// task's brief adds, each only when there is one. `needs_you`: what its agent asked;
    /// `attention`: the reason of its flag; `held_prompt`: a typed prompt Yulius held for a
    /// decision; `pause`: who paused it. The held prompt's words (what a person typed, which
    /// never reached the agent or a transcript) go to `person`'s own callers only, their
    /// Superagent and their own agent: a task's agent learns that a prompt is held, and of
    /// which kind.
    pub(crate) async fn waits_why(
        &self,
        task: &TaskRecord,
        person: bool,
    ) -> serde_json::Map<String, Value> {
        // What the task's summary shows, also for a task that ended: a prompt held before its
        // end is still decided on its card.
        let mut why = serde_json::Map::new();
        let mut add = |key: &str, value: Option<Value>| {
            if let Some(value) = value.filter(|value| !value.is_null()) {
                why.insert(key.to_owned(), value);
            }
        };
        // An ended task waits for nobody (ADR-0111): a request left from before its end is not
        // shown, as its summary shows none.
        let request = if task.status.is_resting() {
            None
        } else {
            self.needs_you_view(&task.id).await
        };
        add(
            "needs_you",
            request.and_then(|view| {
                // A request stored before its words were cleaned is shown as one now is.
                let mut view = serde_json::to_value(view).ok()?;
                for key in ["action", "happened", "problem", "link"] {
                    if let Some(said) = view.get(key).and_then(Value::as_str) {
                        view[key] = json!(crate::attention::shown_line(said, SHOWN_TEXT_CHARS));
                    }
                }
                Some(view)
            }),
        );
        add(
            "attention",
            self.attention_view(task)
                .await
                .and_then(|view| serde_json::to_value(view).ok()),
        );
        add(
            "held_prompt",
            self.held_prompt_view(&task.id).await.map(|held| {
                let mut shown = json!({"class": held.class, "at": held.created_at});
                if person {
                    // What a person typed, as one line: it may hold anything, a line that
                    // reads like another task's among them.
                    shown["excerpt"] = json!(crate::attention::shown_line(
                        &held.excerpt,
                        HELD_EXCERPT_CHARS
                    ));
                    shown["typed_by"] = json!(held.typed_by);
                }
                shown
            }),
        );
        add(
            "pause",
            self.pause_view(task)
                .await
                .and_then(|pause| serde_json::to_value(pause).ok()),
        );
        why
    }

    /// Dismisses what `task` asks of its person (`needs_you`: its agent's request, or one
    /// Yulius opened for it, such as no subscription left), for the task's owner (QST-005):
    /// the request ends without a word to the agent, as when the person has done it elsewhere
    /// or it no longer matters. Only the request that was read is ended: one the
    /// agent made meanwhile stays. Answers what was asked. Audited under the person's name.
    ///
    /// # Errors
    /// `Forbidden` for anyone but the owner; `Conflict` when nothing is asked (a flag says
    /// what ends it instead) or the request changed meanwhile.
    pub async fn dismiss_task_question(&self, ctx: &AuthContext, id: &str) -> AppResult<String> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let store = &self.deps.store;
        if task.status.is_resting() {
            return Err(AppError::Conflict(format!(
                "task #{} asks nothing of a person that can be dismissed: it has ended ({}), \
                 and an ended task waits for nobody",
                task.number,
                task.status.as_str()
            )));
        }
        let Some(request) = store.task_needs_you(&task.id).await? else {
            let flagged = self.attention_view(&task).await.map(|flag| {
                let said = [flag.problem, flag.action]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                match flag.kind.as_str() {
                    "terminal" => format!(
                        ": Claude Code asks in its terminal ({said}), which its person answers \
                         there"
                    ),
                    _ => format!(": Yulius flagged it ({said}), which ends when that is done"),
                }
            });
            return Err(AppError::Conflict(format!(
                "task #{} asks nothing of a person that can be dismissed{}",
                task.number,
                flagged.unwrap_or_default()
            )));
        };
        if !store
            .end_task_needs_you_asking(&task.id, &request.action, self.now())
            .await?
        {
            return Err(AppError::Conflict(format!(
                "task #{}'s request changed meanwhile: read it again (task_get)",
                task.number
            )));
        }
        // The request ended: a failed audit write is logged, not reported as a failed call.
        if let Err(err) = store
            .audit(
                self.now(),
                &self.login_of(&ctx.user.id).await,
                "task.needs_you.dismissed",
                Some(task.id.as_str()),
                Some(&request.action),
            )
            .await
        {
            tracing::warn!(task = %task.id, error = %err, "a dismissed request was not audited");
        }
        self.publish_task(&task.id).await;
        // What was asked, as the brief shows it.
        Ok(crate::attention::shown_line(&request.action, SHOWN_TEXT_CHARS).unwrap_or_default())
    }

    /// Decides the prompt Yulius held in `task` (TSK-013), for the task's owner, as its card
    /// does (QST-005): `action` is one of [`HELD_PROMPT_ACTIONS`]. The page's function runs,
    /// with its checks and its audit.
    ///
    /// # Errors
    /// `Invalid` for another action; `Forbidden` for anyone but the owner; `Conflict` when the
    /// task holds no prompt; the page's errors.
    pub async fn decide_task_held_prompt(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        action: Option<&str>,
    ) -> AppResult<HeldPromptOutcome> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let action = action
            .filter(|action| HELD_PROMPT_ACTIONS.contains(action))
            .ok_or_else(|| {
                AppError::Invalid(
                    "`action` is send, dismiss, parallel or after (skipping steps and reverting \
                     the task are decided on the task's page)"
                        .into(),
                )
            })?;
        let hold = self
            .deps
            .store
            .pending_hold(&task.id)
            .await?
            .ok_or_else(|| {
                AppError::Conflict(format!("task #{} holds no prompt now", task.number))
            })?;
        let req = ResolveHeldPrompt {
            action: action.to_owned(),
            skip_step_ids: Vec::new(),
            confirm: false,
        };
        self.resolve_held_prompt(ctx, id, &hold.id, &req).await
    }
}

#[cfg(test)]
mod tests {
    use super::group_name;
    use yulius_domain::task::TaskGroup;

    #[test]
    fn every_group_has_its_name() {
        let names: Vec<&str> = [
            TaskGroup::NeedsYou,
            TaskGroup::Working,
            TaskGroup::Queued,
            TaskGroup::Done,
            TaskGroup::Ended,
        ]
        .into_iter()
        .map(group_name)
        .collect();
        assert_eq!(names, ["needs_you", "working", "queued", "done", "ended"]);
    }
}
