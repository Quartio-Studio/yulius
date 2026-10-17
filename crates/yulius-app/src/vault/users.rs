//! Who uses agent secrets through the vault's MCP tools (SPEC-186, ADR-0150): a task's agent, in
//! its worktree, or a person's Superagent, in the sandbox repository it runs in. Both get the same
//! tools and safeguards; what differs is where a `.env` copy goes, how a person sees who asked,
//! and whether a task is flagged as needing a person.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use yulius_domain::ids::SandboxId;
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::context::App;
use crate::coordination::{AgentContext, ProjectCaller};
use crate::error::AppResult;
use crate::model::{SandboxRecord, TaskRecord};
use crate::ports::vault::EnvFiles;

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

    /// How a person sees a task that asked: `#<n>`.
    pub(crate) fn task_asker(number: u32) -> String {
        format!("#{number}")
    }

    /// How a person sees who asked: `#<n>`, or `<login>'s Superagent`.
    pub(crate) fn asker(&self) -> String {
        match self {
            Self::Task(ctx) => Self::task_asker(ctx.task.number),
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
    /// person is in front of it, so nothing is flagged. A flag it raises itself is remembered,
    /// so the person's answer ends it ([`Self::settle_secret_waits`]); one set before is not
    /// its own (SPEC-213 SRA-003).
    ///
    /// # Errors
    /// Store failures reading the task.
    pub(super) async fn flag_secret_wait(&self, user: SecretUser<'_>, what: &str) -> AppResult<()> {
        let SecretUser::Task(ctx) = user else {
            return Ok(());
        };
        let Some(task) = self.deps.store.task(&ctx.task.id).await? else {
            return Ok(());
        };
        if task.activity == Some(Activity::NeedsAttention) {
            return Ok(());
        }
        if let Err(err) = self.raise_secret_flag(&task).await {
            tracing::warn!(task = %task.id, error = %err, "could not flag {what}");
        }
        Ok(())
    }

    /// Flags `task` for its secret request and remembers the flag's state time as its own.
    async fn raise_secret_flag(&self, task: &TaskRecord) -> AppResult<()> {
        self.set_task_activity(task, Activity::NeedsAttention)
            .await?;
        if let Some(times) = self.deps.store.task_times(&task.id).await? {
            self.runtime(&task.id)
                .secret_wait_at
                .store(times.state_at.0, Ordering::SeqCst);
        }
        Ok(())
    }

    /// A person answered a request for a secret of `sandbox` (`None`: of every sandbox), or
    /// deleted the secret: each running task still flagged by its own secret request, with no
    /// request waiting for a person any more, works on (SPEC-213 SRA-001, SRA-002). Failures are
    /// logged per task: the answer stands.
    pub(super) async fn settle_secret_waits(&self, sandbox: Option<&SandboxId>) {
        let store = &self.deps.store;
        let tasks = match sandbox {
            Some(sandbox) => store.tasks(sandbox, false).await,
            None => store.tasks_with_status(&[TaskStatus::Running]).await,
        };
        let tasks = tasks.unwrap_or_else(|err| {
            tracing::warn!(error = %err, "could not end the flags of answered secret requests");
            Vec::new()
        });
        for task in tasks {
            if task.status == TaskStatus::Running
                && task.activity == Some(Activity::NeedsAttention)
                && let Err(err) = self.settle_secret_wait(&task).await
            {
                tracing::warn!(task = %task.id, error = %err, "could not end the flag of its answered secret request");
            }
        }
    }

    /// Ends `task`'s flag if its own secret request raised it, still stands, and nothing waits.
    async fn settle_secret_wait(&self, task: &TaskRecord) -> AppResult<()> {
        let Some(runtime) = self.existing_runtime(&task.id) else {
            return Ok(());
        };
        let flagged = runtime.secret_wait_at.load(Ordering::SeqCst);
        if flagged == 0 || self.secret_waits_for(task).await? {
            return Ok(());
        }
        // Only the flag it raised (SRA-003); a Stop that settled the task meanwhile stays.
        let from = (Activity::NeedsAttention, Timestamp(flagged));
        let now = self.now();
        if !self
            .deps
            .store
            .set_activity_from(&task.id, from, Some(Activity::Working), now)
            .await?
        {
            return Ok(());
        }
        runtime.secret_wait_at.store(0, Ordering::SeqCst);
        tracing::info!(task = %task.id, "its secret requests are answered: the task works on");
        self.publish_task(&task.id).await;
        // The agent asked again while this looked: that request was written before it saw the
        // flag, so it shows here, and flags the task anew.
        if self.secret_waits_for(task).await?
            && let Some(fresh) = self.deps.store.task(&task.id).await?
        {
            self.raise_secret_flag(&fresh).await?;
        }
        Ok(())
    }

    /// Whether a `.env` or host request of `task`'s agent still waits for a person: the task is
    /// among its askers (`#N`, `#N in <slug>` for a secret of every sandbox).
    async fn secret_waits_for(&self, task: &TaskRecord) -> AppResult<bool> {
        let store = &self.deps.store;
        let records = store.vault_agent(Some(&task.sandbox_id)).await?;
        let me = SecretUser::task_asker(task.number);
        // A secret of every sandbox names its askers with their sandbox.
        let everywhere = if records.iter().any(|r| r.sandbox_id.is_none()) {
            let slug = store.sandbox(&task.sandbox_id).await?.map(|s| s.slug);
            super::requests::requester(None, false, &me, Some(&slug.unwrap_or_default()))
        } else {
            String::new()
        };
        Ok(records.iter().any(|record| {
            let asker = if record.sandbox_id.is_some() {
                &me
            } else {
                &everywhere
            };
            (record.env_files == EnvFiles::Asked
                && super::requests::among_askers(record.env_requested_by.as_deref(), asker))
                || (!record.requested_hosts.is_empty()
                    && super::requests::among_askers(record.requested_by.as_deref(), asker))
        }))
    }
}
