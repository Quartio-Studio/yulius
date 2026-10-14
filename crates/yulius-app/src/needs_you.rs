//! Agents ask a person to act (SPEC-134 CDT-004, ADR-0111): `needs_you {action, why?, link?}`
//! records one open request per task. While it is open the task is in the sidebar's Needs you
//! group ("Needs you: <action>") whatever the agent does meanwhile. It ends when a prompt is typed
//! into the task (a person answered, or did it and said so) or the agent calls
//! `needs_you {resolved: true}`.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use yulius_contracts::api::tasks::NeedsYouView;
use yulius_domain::ids::TaskId;
use yulius_domain::time::Timestamp;

use crate::auth::rfc3339;
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};

/// Longest `action`, in characters.
pub const MAX_ACTION_CHARS: usize = 200;
/// Longest `why`, in characters.
pub const MAX_WHY_CHARS: usize = 500;

/// One open request (stored as JSON in `tasks.needs_you`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeedsYouRecord {
    /// What to do.
    pub action: String,
    /// Why.
    #[serde(default)]
    pub why: Option<String>,
    /// Where (a Yulius page).
    #[serde(default)]
    pub link: Option<String>,
    /// When it was asked (ms).
    pub at: i64,
}

impl NeedsYouRecord {
    /// The API view.
    #[must_use]
    pub fn view(&self) -> NeedsYouView {
        NeedsYouView {
            action: self.action.clone(),
            why: self.why.clone(),
            link: self.link.clone(),
            at: rfc3339(Timestamp(self.at)),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    resolved: bool,
}

/// One line of at most `max` characters: control characters become spaces, the rest is trimmed.
fn line(text: &str, max: usize, what: &str) -> AppResult<Option<String>> {
    let clean: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let clean = clean.trim();
    if clean.chars().count() > max {
        return Err(AppError::Invalid(format!(
            "{what} is longer than {max} characters"
        )));
    }
    Ok((!clean.is_empty()).then(|| clean.to_owned()))
}

impl App {
    /// `needs_you`: records what a person should do (or ends the request with `resolved`), flags
    /// the task and tells the people watching it.
    ///
    /// # Errors
    /// `Invalid` arguments (no action, too long, a link outside Yulius).
    pub(crate) async fn tool_needs_you(
        &self,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let args: Args = serde_json::from_value(args.clone())
            .map_err(|e| AppError::Invalid(format!("invalid arguments: {e}")))?;
        let store = &self.deps.store;
        if args.resolved {
            let ended = store.set_task_needs_you(&ctx.task.id, None).await?;
            if ended {
                self.publish_task(&ctx.task.id).await;
                self.needs_you_audit(ctx, "task.needs_you.resolved", "resolved by the agent")
                    .await;
            }
            return Ok(json!({
                "resolved": ended,
                "next": if ended { "The request is closed; the task no longer waits for a person." } else { "No request was open." },
            }));
        }
        let action = line(
            args.action.as_deref().unwrap_or_default(),
            MAX_ACTION_CHARS,
            "action",
        )?
        .ok_or_else(|| {
            AppError::Invalid("say what the person should do in action (one line)".into())
        })?;
        let why = line(
            args.why.as_deref().unwrap_or_default(),
            MAX_WHY_CHARS,
            "why",
        )?;
        let link = match line(args.link.as_deref().unwrap_or_default(), 2_000, "link")? {
            None => None,
            Some(link) => {
                let base = self.config.public_url.trim_end_matches('/');
                if link != base && !link.starts_with(&format!("{base}/")) {
                    return Err(AppError::Invalid(format!(
                        "link must be a page of this Yulius ({base}/…); describe other places in why"
                    )));
                }
                Some(link)
            }
        };
        let record = NeedsYouRecord {
            action: action.clone(),
            why,
            link: link.clone(),
            at: self.now().0,
        };
        // The request itself puts the task in Needs you (`AttentionFacts`, `taskInfo`), whatever the
        // agent does meanwhile; its activity is left to its turns.
        store
            .set_task_needs_you(&ctx.task.id, Some(&record))
            .await?;
        self.publish_task(&ctx.task.id).await;
        self.needs_you_audit(ctx, "task.needs_you", &action).await;
        Ok(json!({
            "asked": action,
            "link": link,
            "next": "People see \"Needs you\" on this task. Print the request (and the link) in the session so whoever opens it knows what to do; go on with other work. It closes when someone writes to this task, or call needs_you with resolved: true once you see it done.",
        }))
    }

    async fn needs_you_audit(&self, ctx: &AgentContext, action: &str, detail: &str) {
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &ctx.actor(),
                action,
                Some(ctx.task.id.as_str()),
                Some(detail),
            )
            .await
        {
            tracing::warn!(error = %err, task = %ctx.task.id, "needs_you audit failed");
        }
    }

    /// A prompt typed into the task ends its request: a person answered.
    pub(crate) async fn end_needs_you(&self, task: &TaskId) {
        match self.deps.store.set_task_needs_you(task, None).await {
            Ok(true) => self.publish_task(task).await,
            Ok(false) => {}
            Err(err) => {
                tracing::warn!(task = %task, error = %err, "ending a needs_you request failed")
            }
        }
    }

    /// The task's open request, for its summary.
    pub(crate) async fn needs_you_view(&self, task: &TaskId) -> Option<NeedsYouView> {
        match self.deps.store.task_needs_you(task).await {
            Ok(request) => request.map(|r| r.view()),
            Err(err) => {
                tracing::warn!(task = %task, error = %err, "reading a needs_you request failed");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_single_bounded_lines() {
        assert_eq!(
            line("  Add DEV_SSH_KEY\non Secrets ", 200, "action")
                .ok()
                .flatten(),
            Some("Add DEV_SSH_KEY on Secrets".to_owned())
        );
        assert_eq!(line("   ", 200, "action").ok().flatten(), None);
        assert!(line(&"a".repeat(201), 200, "action").is_err());
    }
}
