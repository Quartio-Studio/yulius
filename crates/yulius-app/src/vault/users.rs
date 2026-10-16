//! Who uses agent secrets through the vault's MCP tools (SPEC-186, ADR-0150): a task's agent, in
//! its worktree, or a person's Superagent, in the sandbox repository it runs in. Both get the same
//! tools and safeguards; what differs is where a `.env` copy goes, how a person sees who asked,
//! and whether a task is flagged as needing a person.

use std::path::PathBuf;

use yulius_domain::ids::SandboxId;
use yulius_domain::task::Activity;

use crate::context::App;
use crate::coordination::{AgentContext, ProjectCaller};
use crate::error::AppResult;
use crate::model::SandboxRecord;

/// A caller of the vault tools that use a secret.
#[derive(Clone, Copy)]
pub(crate) enum SecretUser<'a> {
    /// A task's agent.
    Task(&'a AgentContext),
    /// A person's Superagent (its `ck_` caller; no task).
    Superagent(&'a ProjectCaller),
}

impl SecretUser<'_> {
    /// Its project's sandbox.
    pub(crate) fn sandbox(&self) -> &SandboxId {
        match self {
            Self::Task(ctx) => &ctx.task.sandbox_id,
            Self::Superagent(caller) => &caller.sandbox,
        }
    }

    /// The audit actor: `task:<n>`, or `cockpit:<login>`.
    pub(crate) fn actor(&self) -> String {
        match self {
            Self::Task(ctx) => ctx.actor(),
            Self::Superagent(caller) => caller.actor.clone(),
        }
    }

    /// What its uses are audited once per: the agent's run, or the Superagent's process.
    pub(crate) fn run(&self) -> String {
        match self {
            Self::Task(ctx) => ctx.attempt.id.as_str().to_owned(),
            Self::Superagent(caller) => format!(
                "cockpit:{}",
                caller.detail["shell"].as_str().unwrap_or_default()
            ),
        }
    }

    /// How a person sees who asked: `#<n>`, or `<login>'s Superagent`.
    pub(crate) fn asker(&self) -> String {
        match self {
            Self::Task(ctx) => format!("#{}", ctx.task.number),
            Self::Superagent(caller) => {
                let login = caller
                    .actor
                    .strip_prefix("cockpit:")
                    .unwrap_or(&caller.actor);
                format!("{login}'s Superagent")
            }
        }
    }
}

impl App {
    /// The folder a caller's `.env` copies go into: a task's worktree, or the sandbox repository
    /// the Superagent runs in.
    pub(super) async fn secret_user_folder(
        &self,
        user: SecretUser<'_>,
        sandbox: &SandboxRecord,
    ) -> AppResult<PathBuf> {
        match user {
            SecretUser::Task(ctx) => {
                self.sandbox_worktree(sandbox, ctx.task.id.as_str(), None)
                    .await
            }
            SecretUser::Superagent(_) => {
                let repo = self.sandbox_repo(&sandbox.id);
                Ok(std::fs::canonicalize(&repo).unwrap_or(repo))
            }
        }
    }

    /// Flags a task whose agent waits for a person (`what` names it in the log); a Superagent's
    /// person is in front of it, so nothing is flagged.
    ///
    /// # Errors
    /// Store failures reading the task.
    pub(super) async fn flag_secret_wait(&self, user: SecretUser<'_>, what: &str) -> AppResult<()> {
        let SecretUser::Task(ctx) = user else {
            return Ok(());
        };
        if let Some(task) = self.deps.store.task(&ctx.task.id).await?
            && let Err(err) = self
                .set_task_activity(&task, Activity::NeedsAttention)
                .await
        {
            tracing::warn!(task = %task.id, error = %err, "could not flag {what}");
        }
        Ok(())
    }
}
