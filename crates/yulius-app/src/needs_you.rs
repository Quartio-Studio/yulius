//! Agents ask a person to act (SPEC-134 CDT-004, ADR-0111):
//! `needs_you {happened?, problem?, action, decision?, link?}` records one open request per task,
//! read as What happened, Problem and To do (or Decide) (SPEC-327, ADR-0221). While it is open the task is in the sidebar's Needs you
//! group ("Needs you: `<action>`") whatever the agent does meanwhile. It ends when a prompt is typed
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
/// Longest `happened`, in characters.
pub const MAX_HAPPENED_CHARS: usize = 300;
/// Longest `problem`, in characters.
pub const MAX_PROBLEM_CHARS: usize = 500;

/// One open request (stored as JSON in `tasks.needs_you`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeedsYouRecord {
    /// What to do, or what to decide when `decision`.
    pub action: String,
    /// What happened.
    #[serde(default)]
    pub happened: Option<String>,
    /// What is wrong or blocked because of it (`why` in rows written before SPEC-327).
    #[serde(default, alias = "why")]
    pub problem: Option<String>,
    /// The person chooses rather than does.
    #[serde(default)]
    pub decision: bool,
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
            happened: self.happened.clone(),
            problem: self.problem.clone(),
            decision: self.decision,
            link: self.link.clone(),
            at: rfc3339(Timestamp(self.at)),
        }
    }

    /// The request as text for the agent to print in its session: a heading, then What
    /// happened, Problem and To do (or Decide), each on its own line when given.
    #[must_use]
    pub fn printed(&self) -> String {
        let mut text = String::from(if self.decision {
            "Needs your decision"
        } else {
            "Needs you"
        });
        for (label, part) in [
            ("What happened", self.happened.as_deref()),
            ("Problem", self.problem.as_deref()),
            (
                if self.decision { "Decide" } else { "To do" },
                Some(self.action.as_str()),
            ),
        ] {
            if let Some(part) = part {
                text.push_str(&format!("\n{label}: {part}"));
            }
        }
        text
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    happened: Option<String>,
    #[serde(default)]
    problem: Option<String>,
    /// The older name of `problem` (agents started before SPEC-327 keep the old schema).
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    decision: bool,
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
            let ended = store
                .set_task_needs_you(&ctx.task.id, None, self.now())
                .await?;
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
        let happened = line(
            args.happened.as_deref().unwrap_or_default(),
            MAX_HAPPENED_CHARS,
            "happened",
        )?;
        let problem = match (args.problem.as_deref(), args.why.as_deref()) {
            (Some(_), Some(_)) => {
                return Err(AppError::Invalid(
                    "give the problem once, in problem (why is its older name)".into(),
                ));
            }
            (Some(text), None) => line(text, MAX_PROBLEM_CHARS, "problem")?,
            (None, Some(text)) => line(text, MAX_PROBLEM_CHARS, "why")?,
            (None, None) => None,
        };
        let link = match line(args.link.as_deref().unwrap_or_default(), 2_000, "link")? {
            None => None,
            Some(link) => {
                let base = self.config.public_url.trim_end_matches('/');
                if link != base && !link.starts_with(&format!("{base}/")) {
                    return Err(AppError::Invalid(format!(
                        "link must be a page of this Yulius ({base}/…); describe other places in action or problem"
                    )));
                }
                Some(link)
            }
        };
        let record = NeedsYouRecord {
            action: action.clone(),
            happened,
            problem,
            decision: args.decision,
            link: link.clone(),
            at: self.now().0,
        };
        // The request itself puts the task in Needs you (`AttentionFacts`, `taskInfo`), whatever the
        // agent does meanwhile; its activity is left to its turns.
        store
            .set_task_needs_you(&ctx.task.id, Some(&record), Timestamp(record.at))
            .await?;
        self.publish_task(&ctx.task.id).await;
        self.needs_you_audit(ctx, "task.needs_you", &action).await;
        Ok(json!({
            "asked": action,
            "print": record.printed(),
            "link": link,
            "next": "People see \"Needs you\" on this task. Print the request (print, and the link) in the session as it is so whoever opens it knows what to do; go on with other work. It closes when someone writes to this task, or call needs_you with resolved: true once you see it done.",
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
        match self
            .deps
            .store
            .set_task_needs_you(task, None, self.now())
            .await
        {
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

    #[test]
    fn a_row_stored_with_why_reads_it_as_the_problem() -> Result<(), serde_json::Error> {
        let record: NeedsYouRecord =
            serde_json::from_str(r#"{"action":"Add DEV_SSH_KEY","why":"SSH needs it","at":1}"#)?;
        assert_eq!(record.problem.as_deref(), Some("SSH needs it"));
        assert_eq!(record.happened, None);
        assert!(!record.decision);
        assert_eq!(
            record.printed(),
            "Needs you\nProblem: SSH needs it\nTo do: Add DEV_SSH_KEY"
        );
        Ok(())
    }
}
