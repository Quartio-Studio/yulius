//! Control of a task (SPEC-070, ADR-0060): a task acts on GitHub with its owner's token, so only
//! the owner changes it. Anyone else who may work in the project asks the owner for control; the
//! owner hands it over or declines, and the asker may withdraw. One request waits per task. When
//! the owner can no longer act (disabled, deleted, or without write access to the project), a
//! request passes control at once. A hand-over moves the task's GitHub token, commit identity and
//! authority to the new owner from its next call on, also in a run that goes on.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::json;
use yulius_contracts::api::tasks::TaskSummary;
use yulius_domain::permission::Action;
use yulius_domain::task::TaskStatus;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, TaskRecord, UserRecord};

/// How control passes: on a request the owner accepted, or past an absent owner (R5).
#[derive(Debug, Clone, Copy)]
enum Handover {
    Requested,
    OwnerAbsent,
}

impl App {
    /// `POST /api/tasks/{tid}/control-request`: `ctx`'s person asks the owner for control of the
    /// task. Asking again changes nothing. With the owner absent, control passes at once, whatever
    /// request waits; a waiting asker who can no longer work gives way (R5).
    ///
    /// # Errors
    /// `NotFound`/`Forbidden` without write access to the project; `Conflict` for the owner, or
    /// while another person's request waits (naming them).
    pub async fn request_task_control(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<TaskSummary> {
        let task = self.load_task(ctx, id, Action::View).await?;
        let (sandbox, _) = self
            .require(&ctx.user, &task.sandbox_id, Action::Work)
            .await?;
        if ctx.user.disabled_at.is_some() {
            return Err(AppError::Forbidden("this account is disabled".into()));
        }
        if task.owner == ctx.user.id {
            return Err(AppError::Conflict("you already own this task".into()));
        }
        let store = &self.deps.store;
        let waiting = store.control_request(&task.id).await?;
        let new = waiting.as_ref().is_none_or(|r| r.by != ctx.user.id);
        if self.owner_is_absent(&task, &sandbox).await {
            if new {
                self.audit_control(&task, ctx, "task.control_requested", json!({}))
                    .await;
            }
            self.pass_task_control(&task, &ctx.user, ctx, Handover::OwnerAbsent)
                .await?;
            return self.control_summary(&task).await;
        }
        // An access GitHub cannot tell right now keeps the waiting request (it may still work).
        if let Some(other) = waiting.filter(|r| r.by != ctx.user.id)
            && !self.may_work(&other.by, &task).await.unwrap_or(true)
        {
            self.drop_request(&task, &other.by, ctx).await?;
        }
        if !store
            .request_control(&task.id, &ctx.user.id, self.now())
            .await?
        {
            let asker = match store.control_request(&task.id).await? {
                Some(request) => format!("@{}", self.login_of(&request.by).await),
                None => "someone else".to_owned(),
            };
            return Err(AppError::Conflict(format!(
                "{asker} already asked for control of this task; ask again once @{} has answered",
                self.login_of(&task.owner).await
            )));
        }
        if new {
            self.audit_control(&task, ctx, "task.control_requested", json!({}))
                .await;
        }
        self.publish_task(&task.id).await;
        self.control_summary(&task).await
    }

    /// `POST /api/tasks/{tid}/control-request/accept`: the owner hands the task to the person
    /// whose request waits.
    ///
    /// # Errors
    /// `Forbidden` for anyone but the owner; `Conflict` when no request waits, when the asker can
    /// no longer work in the project (their request is dropped), or when the task changed
    /// meanwhile.
    pub async fn accept_task_control(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<TaskSummary> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let store = &self.deps.store;
        let request = store
            .control_request(&task.id)
            .await?
            .ok_or_else(|| AppError::Conflict("nobody asks for control of this task".into()))?;
        // A hand-over needs a definite yes; an access GitHub cannot tell fails the call and the
        // request waits.
        let asker = match store.user(&request.by).await? {
            Some(user) if self.may_work(&user.id, &task).await? => user,
            _ => {
                self.drop_request(&task, &request.by, ctx).await?;
                return Err(AppError::Conflict(format!(
                    "@{} can no longer work in this project; their request is dropped",
                    self.login_of(&request.by).await
                )));
            }
        };
        self.pass_task_control(&task, &asker, ctx, Handover::Requested)
            .await?;
        self.control_summary(&task).await
    }

    /// Whether `user` may still work in `task`'s project: an enabled account with write access.
    ///
    /// # Errors
    /// When that cannot be told (GitHub unreachable without a usable cached answer, a store
    /// failure): never read as a no.
    async fn may_work(
        &self,
        user: &yulius_domain::ids::UserId,
        task: &TaskRecord,
    ) -> AppResult<bool> {
        match self.deps.store.user(user).await? {
            Some(user) if user.disabled_at.is_none() => {
                match self.require(&user, &task.sandbox_id, Action::Work).await {
                    Ok(_) => Ok(true),
                    // No access, or a GitHub sign-in that can no longer be renewed: they
                    // cannot work (and the caller, often the owner, is not signed out by a 401).
                    Err(
                        AppError::NotFound | AppError::Forbidden(_) | AppError::Unauthenticated,
                    ) => Ok(false),
                    Err(err) => Err(err),
                }
            }
            _ => Ok(false),
        }
    }

    /// Drops `by`'s waiting request because they can no longer work, audited and published.
    async fn drop_request(
        &self,
        task: &TaskRecord,
        by: &yulius_domain::ids::UserId,
        ctx: &AuthContext,
    ) -> AppResult<()> {
        if self
            .deps
            .store
            .drop_control_request(&task.id, by, self.now())
            .await?
        {
            let asker = self.login_of(by).await;
            self.audit_control(task, ctx, "task.control_dropped", json!({"by": asker}))
                .await;
            self.publish_task(&task.id).await;
        }
        Ok(())
    }

    /// `DELETE /api/tasks/{tid}/control-request`: the asker withdraws their request, or the
    /// owner declines it.
    ///
    /// # Errors
    /// `Forbidden` for anyone else; `Conflict` when no request waits or it changed meanwhile.
    pub async fn drop_task_control_request(
        &self,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<TaskSummary> {
        let task = self.load_task(ctx, id, Action::View).await?;
        let store = &self.deps.store;
        let request = store
            .control_request(&task.id)
            .await?
            .ok_or_else(|| AppError::Conflict("nobody asks for control of this task".into()))?;
        let action = if request.by == ctx.user.id {
            "task.control_withdrawn"
        } else {
            // Declining is the owner's, like every other change of the task.
            self.load_task(ctx, id, Action::Work).await?;
            "task.control_declined"
        };
        if !store
            .drop_control_request(&task.id, &request.by, self.now())
            .await?
        {
            return Err(AppError::Conflict(
                "the request changed meanwhile; look again".into(),
            ));
        }
        let asker = self.login_of(&request.by).await;
        self.audit_control(&task, ctx, action, json!({"by": asker}))
            .await;
        self.publish_task(&task.id).await;
        self.control_summary(&task).await
    }

    /// Whether `task`'s owner can no longer act for it: a disabled or deleted account, or one
    /// without write access to the project. A failure to tell (GitHub unreachable without a
    /// usable cache) counts as present: control never passes without the owner by accident.
    async fn owner_is_absent(&self, task: &TaskRecord, sandbox: &SandboxRecord) -> bool {
        match self.deps.store.user(&task.owner).await {
            Ok(Some(owner)) if owner.disabled_at.is_none() => self
                .access_level(&owner, sandbox)
                .await
                .is_ok_and(|level| !level.allows(Action::Work)),
            Ok(_) => true,
            Err(_) => false,
        }
    }

    /// Passes control of `task` from its owner to `to` (R6): whose request waits, or anyone
    /// when the owner is absent. One conditional update, then the worktree's commit identity, the
    /// run's fingerprint file, the terminal notice, the audit entry and the published summary.
    async fn pass_task_control(
        self: &Arc<Self>,
        task: &TaskRecord,
        to: &UserRecord,
        ctx: &AuthContext,
        how: Handover,
    ) -> AppResult<()> {
        let store = &self.deps.store;
        // A launch settles which personal MCP servers its run holds only when it is spawned: a
        // hand-over then could not restart it for the new owner (SPEC-332 MSC-009).
        if matches!(task.status, TaskStatus::Starting | TaskStatus::Switching)
            && self.holds_personal_mcp(task).await?
        {
            return Err(AppError::Conflict(format!(
                "task #{} is starting with @{}'s personal MCP servers; ask again once it runs",
                task.number,
                self.login_of(&task.owner).await
            )));
        }
        let now = self.now();
        let passed = match how {
            Handover::Requested => {
                store
                    .pass_control(&task.id, &task.owner, &to.id, now)
                    .await?
            }
            Handover::OwnerAbsent => {
                store
                    .take_control(&task.id, &task.owner, &to.id, now)
                    .await?
            }
        };
        if !passed {
            return Err(AppError::Conflict(
                "the task changed meanwhile; look again".into(),
            ));
        }
        let passed = store.task(&task.id).await?.ok_or(AppError::NotFound)?;
        if let Some(worktree) = passed.worktree_path.as_deref() {
            let worktree = std::path::Path::new(worktree);
            if worktree.is_dir()
                && let Err(err) = self.set_task_identity(worktree, &passed).await
            {
                tracing::warn!(task = %task.id, error = %err, "new owner's commit identity not set");
            }
        }
        // The run's fallback fingerprints are the new owner's (SPEC-248 FUP-002).
        if let Err(err) = self.rewrite_run_fingerprints(&task.id).await {
            tracing::warn!(task = %task.id, error = %err, "run's fingerprint file not rewritten");
        }
        // A live run keeps the MCP configuration it started with: one holding the previous
        // owner's personal servers resumes with the new owner's (SPEC-332 MSC-009).
        let held = self.existing_runtime(&task.id).and_then(|runtime| {
            runtime
                .personal_mcp_of
                .lock()
                .ok()
                .and_then(|held| held.clone())
        });
        if held.is_some_and(|owner| owner != to.id)
            && let Some(live) = self.deps.store.live_attempt(&task.id).await?
        {
            self.restart_for_new_owner(&task.id, live.account_id).await;
        }
        let (from, to) = (
            self.login_of(&task.owner).await,
            self.login_of(&to.id).await,
        );
        let runtime = self.runtime(&task.id);
        // The App-fallback notice is the new owner's to see again.
        runtime.acting_as_app_said.store(false, Ordering::SeqCst);
        runtime.hub.notice(format!(
            "Control of this task passed from @{from} to @{to}: it now acts on GitHub as @{to}."
        ));
        self.audit_control(
            task,
            ctx,
            "task.control_passed",
            json!({"from": from, "to": to}),
        )
        .await;
        self.publish_task(&task.id).await;
        Ok(())
    }

    /// Resumes a task's live run for its new owner on the same subscription, as a manual switch
    /// does but with nothing typed: past a usage limit, afresh for a first message never taken,
    /// else the same conversation. A run that cannot be restarted is stopped, so the previous
    /// owner's credential never stays with the new owner (fail closed).
    pub(crate) async fn restart_for_new_owner(
        self: &Arc<Self>,
        task: &yulius_domain::ids::TaskId,
        account: Option<yulius_domain::ids::AccountId>,
    ) {
        use crate::supervisor::RunStart;
        // A launch under way reads the task's owner anew and settles its servers itself.
        match self.deps.store.task(task).await {
            Ok(Some(record))
                if matches!(
                    record.status,
                    TaskStatus::Running | TaskStatus::WaitingCapacity
                ) => {}
            Ok(_) => return,
            Err(err) => {
                tracing::warn!(task = %task, error = %err, "task unreadable for the hand-over restart")
            }
        }
        let start = if self.runtime(task).at_limit.load(Ordering::SeqCst) {
            RunStart::AfterLimit
        } else if self.held_opening(task).await {
            RunStart::Fresh { prompt: None }
        } else {
            RunStart::Resume { prompt: None }
        };
        if let Err(err) = self.switch_with(task, account, "handover", start).await {
            tracing::warn!(task = %task, error = %err, "run holding the previous owner's MCP servers not restarted; stopping it");
            if let Err(err) = self.stop_task_now(task).await {
                tracing::error!(task = %task, error = %err, "run holding the previous owner's MCP servers not stopped");
            }
        }
    }

    /// Audits a control change of `task` by `ctx`'s person; a failed write is logged, as the
    /// change already happened.
    async fn audit_control(
        &self,
        task: &TaskRecord,
        ctx: &AuthContext,
        action: &str,
        mut detail: serde_json::Value,
    ) {
        detail["task"] = json!(format!("#{} {}", task.number, task.title));
        let actor = self.login_of(&ctx.user.id).await;
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &actor,
                action,
                Some(task.id.as_str()),
                Some(&detail.to_string()),
            )
            .await
        {
            tracing::warn!(task = %task.id, error = %err, action, "control audit failed");
        }
    }

    async fn control_summary(&self, task: &TaskRecord) -> AppResult<TaskSummary> {
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.task_summary(&task).await
    }
}
