//! People manage a project's script watches (SPEC-630 WCH-011, WCH-012): the Watches page, the
//! HTTP API and the person tools all call these functions with the person's [`AuthContext`].
//!
//! Contract:
//! - anyone who may view the project lists its watches;
//! - one who may configure it (write access) creates one and becomes its owner;
//! - the owner edits, runs now, pauses and resumes it; the owner or a project administrator pauses
//!   or removes it;
//! - a removal cancels its going run and removes its notices; each change is audited.

use std::collections::HashMap;
use std::sync::Arc;

use yulius_contracts::api::watches::{WatchInput, WatchView};
use yulius_domain::ids::SandboxId;
use yulius_domain::notice::NoticeKey;
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::watch::{WatchName, short};

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::store_ext::WatchRecord;
use crate::watches::{NewWatch, checked_definition, next_due};

/// What a caller may do with one watch.
struct Rights {
    /// Its owner, with a token that may change things.
    edit: bool,
    /// Its owner or a project administrator, with such a token.
    remove: bool,
}

fn new_watch(
    sandbox: &SandboxId,
    ctx: &AuthContext,
    input: WatchInput,
    via: &'static str,
) -> NewWatch {
    NewWatch {
        sandbox: sandbox.clone(),
        owner: ctx.user.id.clone(),
        name: input.name.unwrap_or_default(),
        script: input.script,
        every: input.every.filter(|e| !e.trim().is_empty()),
        cron: input.cron.filter(|c| !c.trim().is_empty()),
        utc_offset_minutes: input.utc_offset_minutes,
        expiry: input.expiry,
        timeout_seconds: input.timeout_seconds,
        secrets: input.secrets,
        created_via: via,
    }
}

impl App {
    /// The project of `sid` the caller may act on as `action`, with their level.
    async fn watch_project(
        &self,
        ctx: &AuthContext,
        sid: &SandboxId,
        action: Action,
    ) -> AppResult<(SandboxRecord, AccessLevel)> {
        let (sandbox, level) = self.require_for(ctx, sid, action).await?;
        if ctx.scope().is_some_and(|scope| !scope.allows(action)) {
            return Err(AppError::Forbidden(
                "this token may only read the project".into(),
            ));
        }
        Ok((sandbox, level))
    }

    /// Watch `id` with its project, when the caller may see it (`NotFound` otherwise).
    async fn watch_seen(
        &self,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<(WatchRecord, SandboxRecord, AccessLevel)> {
        let watch = self.deps.store.watch(id).await?.ok_or(AppError::NotFound)?;
        let (sandbox, level) = self
            .watch_project(ctx, &watch.sandbox_id, Action::View)
            .await?;
        if sandbox.removed_at.is_some() {
            return Err(AppError::NotFound);
        }
        Ok((watch, sandbox, level))
    }

    fn watch_rights(ctx: &AuthContext, watch: &WatchRecord, level: AccessLevel) -> Rights {
        let changes = ctx
            .scope()
            .is_none_or(|scope| scope.allows(Action::Configure));
        let owner = watch.owner == ctx.user.id && level.allows(Action::Configure);
        Rights {
            edit: changes && owner,
            remove: changes && (owner || level.allows(Action::Administer)),
        }
    }

    /// The watch as people see it.
    async fn watch_view(
        &self,
        ctx: &AuthContext,
        watch: &WatchRecord,
        level: AccessLevel,
        pending: &HashMap<String, u32>,
    ) -> WatchView {
        let rights = Self::watch_rights(ctx, watch, level);
        let state = if watch.state.running.is_some() {
            "running"
        } else if watch.ended_reason.is_some() {
            "ended"
        } else if watch.paused {
            "paused"
        } else {
            "active"
        };
        WatchView {
            id: watch.id.clone(),
            sandbox_id: watch.sandbox_id.to_string(),
            name: watch.name.clone(),
            script: watch.script.clone(),
            every: watch.every_seconds.map(short),
            cron: watch.cron.clone(),
            utc_offset_minutes: watch.utc_offset_minutes,
            expiry: watch.expiry.clone(),
            timeout_seconds: watch.timeout_seconds,
            secrets: watch.secrets.clone(),
            owner_id: watch.owner.to_string(),
            owner: self.login_of(&watch.owner).await,
            created_via: watch.created_via.clone(),
            created_at: rfc3339(watch.created_at),
            state: state.into(),
            ended_reason: watch.ended_reason.clone(),
            next_run_at: next_due(watch).map(rfc3339),
            last_started_at: watch.state.last_started_at.map(rfc3339),
            last_ended_at: watch.state.last_ended_at.map(rfc3339),
            last_exit: watch.state.last_exit,
            last_failure: watch.state.last_failure.clone(),
            last_output: watch.state.last_output.clone(),
            last_errors: watch.state.last_errors.clone(),
            failures: watch.state.failures,
            runs: watch.state.runs,
            posts: watch.state.posts,
            pending_posts: pending.get(&watch.id).copied().unwrap_or(0),
            can_edit: rights.edit,
            can_remove: rights.remove,
        }
    }

    async fn watch_pending_counts(&self) -> AppResult<HashMap<String, u32>> {
        let mut counts = HashMap::new();
        for pending in self.deps.store.watch_pending().await? {
            *counts.entry(pending.watch_id).or_insert(0) += 1;
        }
        Ok(counts)
    }

    async fn audit_watch(&self, ctx: &AuthContext, watch: &WatchRecord, action: &str) {
        let login = self.login_of(&ctx.user.id).await;
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &login,
                action,
                Some(watch.sandbox_id.as_str()),
                Some(&format!("{} ({})", watch.name, watch.id)),
            )
            .await
        {
            tracing::warn!(error = %err, action, "a watch's audit entry was not written");
        }
    }

    /// `GET /api/sandboxes/{sid}/watches`: the project's watches, by name.
    ///
    /// # Errors
    /// `NotFound` without view access.
    pub async fn watches_for(&self, ctx: &AuthContext, sid: &str) -> AppResult<Vec<WatchView>> {
        let sid = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        let (sandbox, level) = self.watch_project(ctx, &sid, Action::View).await?;
        let pending = self.watch_pending_counts().await?;
        let mut views = Vec::new();
        for watch in self.deps.store.watches_in(&sandbox.id).await? {
            views.push(self.watch_view(ctx, &watch, level, &pending).await);
        }
        Ok(views)
    }

    /// `GET /api/watches/{id}`.
    ///
    /// # Errors
    /// `NotFound` when it is gone or not the caller's to see.
    pub async fn watch_for(&self, ctx: &AuthContext, id: &str) -> AppResult<WatchView> {
        let (watch, _, level) = self.watch_seen(ctx, id).await?;
        let pending = self.watch_pending_counts().await?;
        Ok(self.watch_view(ctx, &watch, level, &pending).await)
    }

    /// `POST /api/sandboxes/{sid}/watches`: a new watch, owned by the caller (`via`: `ui` or
    /// `agent`).
    ///
    /// # Errors
    /// `NotFound`/`Forbidden` without write access, `Invalid` for a bad definition, `Conflict`
    /// for a name taken or a full project.
    pub async fn watch_create_for(
        &self,
        ctx: &AuthContext,
        sid: &str,
        input: WatchInput,
        via: &'static str,
    ) -> AppResult<WatchView> {
        let sid = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        let (sandbox, level) = self.watch_project(ctx, &sid, Action::Configure).await?;
        let watch = self
            .watch_create(new_watch(&sandbox.id, ctx, input, via))
            .await?;
        self.audit_watch(ctx, &watch, "watch.create").await;
        self.publish_watches(&sandbox.id);
        Ok(self.watch_view(ctx, &watch, level, &HashMap::new()).await)
    }

    /// `PUT /api/watches/{id}`: its owner changes its script, schedule, expiry, timeout or
    /// secrets. Its name never changes (its notices' key).
    ///
    /// # Errors
    /// `Forbidden` for anyone but its owner, `Invalid` for a bad definition or another name,
    /// `Conflict` when it changed meanwhile.
    pub async fn watch_update_for(
        &self,
        ctx: &AuthContext,
        id: &str,
        input: WatchInput,
    ) -> AppResult<WatchView> {
        let (watch, _, level) = self.watch_seen(ctx, id).await?;
        if !Self::watch_rights(ctx, &watch, level).edit {
            return Err(AppError::Forbidden("only its owner changes a watch".into()));
        }
        if input
            .name
            .as_deref()
            .is_some_and(|name| name.trim() != watch.name)
        {
            return Err(AppError::Invalid(
                "a watch's name is the key of its notices and does not change: remove it and \
                 create one with the new name"
                    .into(),
            ));
        }
        let new = new_watch(&watch.sandbox_id, ctx, input, "ui");
        let checked = checked_definition(&new)?;
        let now = self.now();
        let schedule_changed = checked.every_seconds != watch.every_seconds
            || checked.cron != watch.cron
            || checked.utc_offset_minutes != watch.utc_offset_minutes;
        let edited = WatchRecord {
            script: new.script,
            every_seconds: checked.every_seconds,
            cron: checked.cron,
            utc_offset_minutes: checked.utc_offset_minutes,
            expiry: checked.expiry,
            timeout_seconds: checked.timeout_seconds,
            secrets: checked.secrets,
            updated_at: now,
            // A new schedule counts from now; an interval keeps its last start.
            since: if schedule_changed { now } else { watch.since },
            ..watch.clone()
        };
        if !self
            .deps
            .store
            .update_watch(&edited, watch.updated_at)
            .await?
        {
            return Err(AppError::Conflict(
                "the watch changed meanwhile: read it again".into(),
            ));
        }
        self.audit_watch(ctx, &edited, "watch.update").await;
        self.publish_watches(&edited.sandbox_id);
        self.watch_for(ctx, id).await
    }
}

impl App {
    /// `POST /api/watches/{id}/run`: its owner runs it once now, also when it is paused or ended;
    /// it posts as usual. The run goes on after the answer.
    ///
    /// # Errors
    /// `Forbidden` for anyone but its owner, `Conflict` while a run of it goes.
    pub async fn watch_run_now(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<WatchView> {
        let (watch, _, level) = self.watch_seen(ctx, id).await?;
        if !Self::watch_rights(ctx, &watch, level).edit {
            return Err(AppError::Forbidden(
                "only its owner runs a watch now".into(),
            ));
        }
        if watch.state.running.is_some() {
            return Err(AppError::Conflict("a run of this watch is going".into()));
        }
        self.audit_watch(ctx, &watch, "watch.run").await;
        let app = Arc::clone(self);
        let run = watch.id.clone();
        let permits = Arc::clone(&self.watches.permits);
        tokio::spawn(async move {
            // Within the instance's bound, after the runs already going.
            let Ok(_permit) = permits.acquire_owned().await else {
                return;
            };
            app.watch_run(&run, true).await;
        });
        let pending = self.watch_pending_counts().await?;
        Ok(self.watch_view(ctx, &watch, level, &pending).await)
    }

    /// `POST /api/watches/{id}/pause` and `/resume`: its owner pauses or resumes it, and a project
    /// administrator may pause it too. A resume starts an ended watch again; its schedule counts
    /// from now.
    ///
    /// # Errors
    /// `Forbidden` for anyone else.
    pub async fn watch_set_paused_for(
        &self,
        ctx: &AuthContext,
        id: &str,
        paused: bool,
    ) -> AppResult<WatchView> {
        let (watch, _, level) = self.watch_seen(ctx, id).await?;
        let rights = Self::watch_rights(ctx, &watch, level);
        if !(rights.edit || (paused && rights.remove)) {
            return Err(AppError::Forbidden(if paused {
                "only its owner or a project administrator pauses a watch".into()
            } else {
                "only its owner resumes a watch".into()
            }));
        }
        if !self
            .deps
            .store
            .set_watch_paused(&watch.id, paused, self.now())
            .await?
        {
            return Err(AppError::NotFound);
        }
        self.audit_watch(
            ctx,
            &watch,
            if paused {
                "watch.pause"
            } else {
                "watch.resume"
            },
        )
        .await;
        self.publish_watches(&watch.sandbox_id);
        self.watch_for(ctx, id).await
    }

    /// `DELETE /api/watches/{id}`: its owner or a project administrator removes it; its going run
    /// is cancelled and its notices removed.
    ///
    /// # Errors
    /// `Forbidden` for anyone else.
    pub async fn watch_remove_for(&self, ctx: &AuthContext, id: &str) -> AppResult<()> {
        let (watch, _, level) = self.watch_seen(ctx, id).await?;
        if !Self::watch_rights(ctx, &watch, level).remove {
            return Err(AppError::Forbidden(
                "only its owner or a project administrator removes a watch".into(),
            ));
        }
        if !self.deps.store.delete_watch(&watch.id).await? {
            return Err(AppError::NotFound);
        }
        self.watch_cancel(&watch.id, "the watch was removed");
        if let Ok(name) = WatchName::parse(&watch.name) {
            for key in [name.report_key(), name.warning_key()] {
                if let Ok(key) = NoticeKey::parse(&key) {
                    let _ = self
                        .remove_notice(Some(&watch.sandbox_id), Some(&watch.owner), &key)
                        .await;
                }
            }
        }
        self.audit_watch(ctx, &watch, "watch.remove").await;
        self.publish_watches(&watch.sandbox_id);
        Ok(())
    }
}
