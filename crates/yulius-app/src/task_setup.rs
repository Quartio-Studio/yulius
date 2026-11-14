//! What a task runs on, for its person (SPEC-487 slice 6, SUB-001 to SUB-004; wall 5): the
//! subscriptions, a task moved to the one its person names, and a task's model and effort.
//!
//! Contract:
//! - each is a person's function: their [`AuthContext`], the task's owner only for a change;
//! - a move is the page's Switch (`App::switch_task`), with the subscription named by its label
//!   as people name it, or its id;
//! - a model or effort is stored with the task and applies when its session next starts (a
//!   start, a continue, a resume, a move): a running session keeps the one it started with.

use std::sync::Arc;

use serde_json::{Value, json};
use yulius_contracts::api::tasks::{ContinueTask, TaskSummary};
use yulius_domain::permission::Action;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::store::TaskFieldUpdate;

/// The longest model name a task takes (as a launch takes it).
const MODEL_MAX: usize = 128;
/// The longest effort a task takes.
const EFFORT_MAX: usize = 32;

/// Whether `model` has the shape of a model name a launch takes: letters, digits and
/// `._:/@[]-`, not starting with `-` (it goes on Claude Code's command line).
#[must_use]
pub fn model_shaped(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= MODEL_MAX
        && !model.starts_with('-')
        && model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/@[]-".contains(&b))
}

/// Whether `effort` has the shape of an effort level a launch takes.
#[must_use]
pub fn effort_shaped(effort: &str) -> bool {
    !effort.is_empty()
        && effort.len() <= EFFORT_MAX
        && !effort.starts_with('-')
        && effort
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// A model or effort as asked for: `None` when not asked; `Some(None)` for the instance's
/// default (`default` or blank); else the value, if it has the shape `shaped` takes.
fn setting(
    value: Option<&str>,
    what: &str,
    shaped: fn(&str) -> bool,
) -> AppResult<Option<Option<String>>> {
    let Some(value) = value.map(str::trim) else {
        return Ok(None);
    };
    if value.is_empty() || value.eq_ignore_ascii_case("default") {
        return Ok(Some(None));
    }
    if !shaped(value) {
        return Err(AppError::Invalid(format!(
            "the {what} `{}` is not one a launch takes",
            value.chars().take(40).collect::<String>()
        )));
    }
    Ok(Some(Some(value.to_owned())))
}

impl App {
    /// The label of the subscription `task`'s live run is on, if it has one (a removed
    /// subscription is no task's, as in its summary).
    pub(crate) async fn run_subscription(&self, task: &crate::model::TaskRecord) -> Option<String> {
        let store = &self.deps.store;
        let attempt = store.live_attempt(&task.id).await.ok()??;
        let account = store.account(attempt.account_id.as_ref()?).await.ok()??;
        Some(account.label)
    }

    /// Moves task `id` to the subscription named `subscription` (its label, any case, or its
    /// id), or to the pool's best other one when none is named (SUB-003), for the task's owner:
    /// the page's Switch.
    ///
    /// # Errors
    /// `Forbidden` for anyone but the owner; `Invalid` for a name that is no subscription or two
    /// (naming the ones there are); the Switch's refusals.
    pub async fn switch_task_for_person(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        subscription: Option<&str>,
    ) -> AppResult<TaskSummary> {
        self.load_task(ctx, id, Action::Work).await?;
        let account_id = match subscription.map(str::trim).filter(|name| !name.is_empty()) {
            None => None,
            Some(name) => {
                let accounts = self.deps.store.accounts().await?;
                let named: Vec<_> = accounts
                    .iter()
                    .filter(|account| {
                        account.id.as_str() == name
                            || account.label.trim().eq_ignore_ascii_case(name)
                    })
                    .collect();
                match named.as_slice() {
                    [one] => Some(one.id.to_string()),
                    [] => {
                        let labels: Vec<&str> = accounts
                            .iter()
                            .map(|account| account.label.as_str())
                            .collect();
                        return Err(AppError::Invalid(format!(
                            "no subscription is named `{}`; there are: {}",
                            name.chars().take(80).collect::<String>(),
                            labels.join(", ")
                        )));
                    }
                    _ => {
                        return Err(AppError::Invalid(format!(
                            "two subscriptions or more are named `{name}`: name it by its id \
                             (subscriptions lists the ids for an instance administrator) or ask \
                             an administrator to rename one"
                        )));
                    }
                }
            }
        };
        self.switch_task(ctx, id, &ContinueTask { account_id })
            .await
    }

    /// Sets task `id`'s model, effort or both for its owner (SUB-004): `default` (or blank)
    /// goes back to the instance's default. Stored with the task; its session takes it at its
    /// next start, so a running session keeps the one it started with until then. No page
    /// changes it after a task is created. Audited under the person's name.
    ///
    /// # Errors
    /// `Invalid` for neither or a value of a shape a launch does not take; `Forbidden` for
    /// anyone but the owner.
    pub async fn set_task_model(
        &self,
        ctx: &AuthContext,
        id: &str,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> AppResult<Value> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let model = setting(model, "model", model_shaped)?;
        let effort = setting(effort, "effort", effort_shaped)?;
        if model.is_none() && effort.is_none() {
            return Err(AppError::Invalid(
                "give `model` or `effort` (or both); `default` goes back to the default".into(),
            ));
        }
        let update = TaskFieldUpdate {
            model: model.clone(),
            effort: effort.clone(),
            ..TaskFieldUpdate::default()
        };
        self.deps
            .store
            .update_task(&task.id, &update, self.now())
            .await?;
        let detail = json!({"model": model, "effort": effort}).to_string();
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &self.login_of(&ctx.user.id).await,
                "task.model_changed",
                Some(task.id.as_str()),
                Some(&detail),
            )
            .await
        {
            tracing::warn!(task = %task.id, error = %err, "a model change was not audited");
        }
        self.publish_task(&task.id).await;
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        let started = self.task_started(&task).await?;
        let note = if !started {
            "Stored: the task runs with it when it starts."
        } else if task.status.is_resting() {
            "Stored: its session takes it when it is continued (task_restart)."
        } else {
            "Stored: its session keeps the model it started with until its next start; \
             task_switch starts it again now, with this one."
        };
        Ok(json!({
            "task": task.number,
            "model": task.model,
            "effort": task.effort,
            "note": note,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{effort_shaped, model_shaped, setting};

    /// SPEC-487 SUB-004: a model and an effort have the shapes a launch takes; `default` and
    /// blank go back to the default.
    #[test]
    fn a_model_and_an_effort_are_what_a_launch_takes() {
        assert!(model_shaped("claude-sonnet-5-5") && model_shaped("claude-opus-4-1[1m]"));
        assert!(!model_shaped("--help") && !model_shaped("a b") && !model_shaped(""));
        assert!(effort_shaped("xhigh") && !effort_shaped("a b") && !effort_shaped("-x"));
        assert_eq!(setting(None, "model", model_shaped).ok(), Some(None));
        assert_eq!(
            setting(Some(" default "), "model", model_shaped).ok(),
            Some(Some(None))
        );
        assert_eq!(
            setting(Some(""), "effort", effort_shaped).ok(),
            Some(Some(None))
        );
        assert_eq!(
            setting(Some("low"), "effort", effort_shaped).ok(),
            Some(Some(Some("low".into())))
        );
        assert!(setting(Some("--rm"), "model", model_shaped).is_err());
    }
}
