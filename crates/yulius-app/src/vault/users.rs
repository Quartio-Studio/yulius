//! Who uses agent secrets and the sandbox browser through the MCP tools (SPEC-186, ADR-0150,
//! SPEC-437): a task's agent, in its worktree, or a person's Superagent, in the sandbox repository
//! it runs in (a Global Superagent in the project it names). Both get the same tools and
//! safeguards; what differs is where a `.env` copy goes, how a person sees who asked or drives the
//! browser, and whether a task is flagged as needing a person.

use std::sync::atomic::Ordering;

use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::task::{Activity, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::attention::AttentionRecord;
use crate::context::App;
use crate::coordination::{AgentContext, ProjectCaller};
use crate::error::AppResult;
use crate::model::TaskRecord;
use crate::ports::vault::EnvFiles;

/// How often a secret request tries to flag a task whose state changes under it.
const RAISE_TRIES: usize = 3;

/// A caller of the vault tools that use a secret, and of the browser tools.
#[derive(Clone, Copy)]
pub(crate) enum SecretUser<'a> {
    /// A task's agent.
    Task(&'a AgentContext),
    /// A person's Superagent (its `ck_` caller, or a Global Superagent's `gs_` one in a project;
    /// no task).
    Superagent(&'a ProjectCaller),
}

/// What a flag raised for a secret request says (SPEC-472 ATN-004): a person answers it on the
/// Secrets page at `link`, not in the task's terminal.
fn secret_reason(link: &str) -> AttentionRecord {
    AttentionRecord::yulius(
        "The agent asked for a secret, and a person has to answer its request.",
        "Answer it on the Secrets page.",
    )
    .on_page(link)
}

impl SecretUser<'_> {
    /// For whom it resolves its project's secrets (SPEC-245 RES-002).
    pub(crate) fn holder(&self) -> super::Holder<'_> {
        match self {
            Self::Task(ctx) => super::Holder::of_task(&ctx.task),
            Self::Superagent(caller) => super::Holder::Person(&caller.user),
        }
    }

    /// Its project's sandbox.
    pub(crate) fn sandbox(&self) -> &SandboxId {
        match self {
            Self::Task(ctx) => &ctx.task.sandbox_id,
            Self::Superagent(caller) => &caller.sandbox,
        }
    }

    /// The audit actor: `task:<n>`, `cockpit:<login>`, or `superagent:<login>` for a Global
    /// Superagent.
    pub(crate) fn actor(&self) -> String {
        match self {
            Self::Task(ctx) => ctx.actor(),
            Self::Superagent(caller) => caller.actor.clone(),
        }
    }

    /// What its uses are audited once per: the agent's run, or the Superagent's process
    /// (`cockpit:<shell>`, a Global Superagent's shell being a Cockpit's too).
    pub(crate) fn run(&self) -> String {
        match self {
            Self::Task(ctx) => ctx.attempt.id.as_str().to_owned(),
            Self::Superagent(caller) => format!(
                "cockpit:{}",
                caller.detail["shell"].as_str().unwrap_or_default()
            ),
        }
    }

    /// Whether it is a Global Superagent, which has no `secret_request_hosts` (SPEC-437).
    pub(crate) fn global(&self) -> bool {
        matches!(self, Self::Superagent(caller) if caller.detail["via"] == "global-superagent")
    }

    /// Its task, `None` for a Superagent.
    pub(crate) fn task(&self) -> Option<&TaskRecord> {
        match self {
            Self::Task(ctx) => Some(&ctx.task),
            Self::Superagent(_) => None,
        }
    }

    /// The label viewers see on its cursor in the browser (BRW-007): `#<n> <harness>`,
    /// `<login>'s Superagent` or `<login>'s Global Superagent` (SPEC-437 SAB-004).
    pub(crate) fn label(&self) -> String {
        match self {
            Self::Task(ctx) => format!("#{} {}", ctx.task.number, ctx.task.harness.display_name()),
            Self::Superagent(caller) if self.global() => {
                let login = caller
                    .actor
                    .strip_prefix("superagent:")
                    .unwrap_or(&caller.actor);
                format!("{login}'s Global Superagent")
            }
            Self::Superagent(_) => self.asker(),
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
    /// Flags a task whose agent waits for a person (`what` names it in the log); a Superagent's
    /// person is in front of it, so nothing is flagged. A flag it raises itself is remembered,
    /// so the person's answer ends it ([`Self::settle_secret_waits`]); one set before is not
    /// its own (SPEC-213 SRA-003). The flag says what its person does and where: on the Secrets
    /// page at `link`, the one the agent prints (SPEC-472); the task's terminal shows no
    /// question for it.
    ///
    /// # Errors
    /// None: a flag that cannot be raised or ended is logged, and the request stands.
    pub(super) async fn flag_secret_wait(
        &self,
        user: SecretUser<'_>,
        what: &str,
        link: &str,
    ) -> AppResult<()> {
        let SecretUser::Task(ctx) = user else {
            return Ok(());
        };
        if let Err(err) = self
            .raise_secret_flag(&ctx.task.id, &secret_reason(link))
            .await
        {
            tracing::warn!(task = %ctx.task.id, error = %err, "could not flag {what}");
        }
        // A person who answered between the request and its flag found no flag to end: it ends
        // now, as nothing waits any more (review round 4).
        let settled = match self.deps.store.task(&ctx.task.id).await {
            Ok(Some(task)) if task.activity == Some(Activity::NeedsAttention) => {
                self.settle_secret_wait(&task).await
            }
            Ok(_) => Ok(()),
            Err(err) => Err(err),
        };
        if let Err(err) = settled {
            tracing::warn!(task = %ctx.task.id, error = %err, "could not end the flag of its answered secret request");
        }
        Ok(())
    }

    /// Flags a running task for its secret request, unless it reads flagged already, and marks
    /// the flag as its own. Under the task's attention lock, so no other flag, request or answer
    /// interleaves; the write is a compare-and-set from the state it read, as a Stop does not
    /// take the lock (SPEC-213 SRA-003, review rounds 2 and 3). The flag carries `reason`.
    async fn raise_secret_flag(&self, task_id: &TaskId, reason: &AttentionRecord) -> AppResult<()> {
        let store = &self.deps.store;
        let runtime = self.runtime(task_id);
        let _held = runtime.attention_lock.lock().await;
        for _ in 0..RAISE_TRIES {
            let (Some(task), Some(times)) =
                (store.task(task_id).await?, store.task_times(task_id).await?)
            else {
                return Ok(());
            };
            if task.status != TaskStatus::Running || task.activity == Some(Activity::NeedsAttention)
            {
                return Ok(());
            }
            let now = self.now();
            let from = (task.activity, times.state_at);
            if store
                .flag_attention(task_id, Some(from), Some(reason), now)
                .await?
            {
                runtime.secret_wait_at.store(now.0, Ordering::SeqCst);
                self.publish_task(task_id).await;
                return Ok(());
            }
        }
        // The task kept changing: flagged all the same, not as the request's (it ends at the
        // turn's end, as before SPEC-213).
        runtime.secret_wait_at.store(0, Ordering::SeqCst);
        if store
            .flag_attention(task_id, None, Some(reason), self.now())
            .await?
        {
            self.publish_task(task_id).await;
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
        let held = runtime.attention_lock.lock().await;
        let flagged = runtime.secret_wait_at.load(Ordering::SeqCst);
        if flagged == 0 {
            return Ok(());
        }
        let waiting = self.waiting_secret_links(task).await?;
        if let Some(first) = waiting.first() {
            // Another request still waits (SRA-002): the flag stays and points at one that
            // does, not at the page of the request just answered (SPEC-472 ATN-011).
            let store = &self.deps.store;
            let shown = store.task_attention(&task.id).await?;
            let shown = shown.and_then(|(reason, _)| reason.link);
            if !shown.is_some_and(|link| waiting.contains(&link))
                && store
                    .replace_attention(
                        &task.id,
                        Timestamp(flagged),
                        Some(&secret_reason(first)),
                        self.now(),
                    )
                    .await?
            {
                drop(held);
                self.publish_task(&task.id).await;
            }
            return Ok(());
        }
        // Only the flag it raised (SRA-003); a Stop that settled the task meanwhile stays.
        let from = (Some(Activity::NeedsAttention), Timestamp(flagged));
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
        drop(held);
        tracing::info!(task = %task.id, "its secret requests are answered: the task works on");
        self.publish_task(&task.id).await;
        // The agent asked again while this looked (its request is written before it flags, which
        // waited for the lock and found the task flagged): it shows here, and flags the task anew.
        if let Some(link) = self.waiting_secret_links(task).await?.first() {
            self.raise_secret_flag(&task.id, &secret_reason(link))
                .await?;
        }
        Ok(())
    }

    /// Where a person answers each `.env`, host or copy request (SPEC-440) of `task`'s agent
    /// that still waits: the Secrets page at the tab of its secret (a copy's request shows on
    /// the production secret). The task is among the request's askers (`#N`, `#N in <slug>` for
    /// a secret of every sandbox). Empty when nothing waits; each page once.
    async fn waiting_secret_links(&self, task: &TaskRecord) -> AppResult<Vec<String>> {
        let store = &self.deps.store;
        let records = store.vault_agent_in(&task.sandbox_id).await?;
        let me = SecretUser::task_asker(task.number);
        let slug = store.sandbox(&task.sandbox_id).await?.map(|s| s.slug);
        let slug = slug.unwrap_or_default();
        // A secret of every sandbox names its askers with their sandbox.
        let everywhere = super::requests::requester(None, false, &me, Some(&slug));
        let mut links = Vec::new();
        // A copy is asked for the task's own project: its askers carry no sandbox.
        for copy in store.vault_copy_requests(&task.sandbox_id).await? {
            if !super::requests::among_askers(Some(&copy.requested_by), &me) {
                continue;
            }
            links.push(match store.vault_get(&copy.secret_id).await? {
                Some(source) => self.agent_secret_link(&slug, &source),
                None => self.agent_secrets_link(&slug),
            });
        }
        for record in &records {
            let asker = if record.sandbox_id.is_some() {
                &me
            } else {
                &everywhere
            };
            if (record.env_files == EnvFiles::Asked
                && super::requests::among_askers(record.env_requested_by.as_deref(), asker))
                || (!record.requested_hosts.is_empty()
                    && super::requests::among_askers(record.requested_by.as_deref(), asker))
            {
                links.push(self.agent_secret_link(&slug, record));
            }
        }
        links.dedup();
        Ok(links)
    }
}
