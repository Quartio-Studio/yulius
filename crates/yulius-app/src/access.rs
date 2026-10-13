//! GitHub-mirrored access decisions with a bounded-staleness cache (SPEC-001 ACC-007/008), the
//! superadmin's access to every sandbox (ACC-012), and whose GitHub token an action uses: a
//! person's own, or for a task its owner's (F 2026-10-11, SPEC-070).

use std::sync::atomic::Ordering;

use yulius_domain::ids::SandboxId;
use yulius_domain::permission::{AccessLevel, Action};

use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, Secret, TaskRecord, UserRecord};
use crate::ports::github::RepoAccess;

impl App {
    /// Effective access of `user` to `sandbox`.
    ///
    /// The superadmin owns the instance and has admin access to every sandbox, with or without
    /// a GitHub link, without asking GitHub (ACC-012). Everyone else mirrors GitHub: fresh cache
    /// entries are reused; otherwise GitHub is asked with the user's token. On a GitHub failure a
    /// cached value younger than the staleness bound is used, else the error is returned. Other
    /// users without a GitHub identity have no access.
    ///
    /// Every access path (REST, the event stream, terminal, shell and browser sockets and their
    /// re-checks, files, Git, tasks and analytics) decides through this function.
    ///
    /// # Errors
    /// `Upstream` when GitHub cannot be reached and no usable cache exists.
    pub async fn access_level(
        &self,
        user: &UserRecord,
        sandbox: &SandboxRecord,
    ) -> AppResult<AccessLevel> {
        if user.disabled_at.is_some() {
            return Ok(AccessLevel::None);
        }
        if user.is_superadmin {
            return Ok(AccessLevel::Admin);
        }
        if user.github_login.is_none() {
            return Ok(AccessLevel::None);
        }
        let now = self.now();
        let store = &self.deps.store;
        let cached = store.cached_access(&user.id, &sandbox.id).await?;
        if let Some(c) = cached
            && now.millis_since(c.checked_at) < self.config.permission_ttl_ms
        {
            return Ok(c.level);
        }
        match self.fetch_access(user, sandbox).await {
            Ok(level) => {
                store.put_access(&user.id, &sandbox.id, level, now).await?;
                Ok(level)
            }
            Err(err) => match cached {
                Some(c) if now.millis_since(c.checked_at) < self.config.permission_stale_ms => {
                    tracing::warn!(error = %err, "using stale permission cache");
                    Ok(c.level)
                }
                _ => Err(err),
            },
        }
    }

    async fn fetch_access(
        &self,
        user: &UserRecord,
        sandbox: &SandboxRecord,
    ) -> AppResult<AccessLevel> {
        let token = self.user_github_token(user).await?;
        match self
            .deps
            .github
            .repo_access(&token, &sandbox.repo_owner, &sandbox.repo_name)
            .await?
        {
            RepoAccess::NoAccess => Ok(AccessLevel::None),
            RepoAccess::Access { permissions, .. } => Ok(AccessLevel::from_github(permissions)),
        }
    }

    /// The user's GitHub access token, refreshed when expired.
    ///
    /// # Errors
    /// `Unauthenticated` when the user never signed in with GitHub or the refresh failed.
    pub async fn user_github_token(&self, user: &UserRecord) -> AppResult<Secret> {
        let store = &self.deps.store;
        let record = store
            .github_token(&user.id)
            .await?
            .ok_or(AppError::Unauthenticated)?;
        let now = self.now();
        let expired = record
            .expires_at
            .is_some_and(|exp| now.plus_millis(60_000) >= exp);
        if !expired {
            let bytes = self.deps.crypto.open(&record.access)?;
            return String::from_utf8(bytes)
                .map(Secret)
                .map_err(AppError::internal);
        }
        let refresh = record.refresh.as_ref().ok_or(AppError::Unauthenticated)?;
        let refresh =
            Secret(String::from_utf8(self.deps.crypto.open(refresh)?).map_err(AppError::internal)?);
        let app = self.github_app().await?;
        let fresh = self
            .deps
            .github
            .refresh_token(&app, &refresh)
            .await
            .map_err(|_| AppError::Unauthenticated)?;
        self.save_user_token(user, &fresh).await?;
        Ok(fresh.access)
    }

    /// A GitHub token for an action that acts as the person on GitHub (merging, releasing).
    /// An account without a GitHub identity (the superadmin, ACC-012) is refused with a message
    /// naming `action` — never `Unauthenticated`, which would end the browser session.
    ///
    /// # Errors
    /// `Forbidden` without a GitHub identity; otherwise as [`App::user_github_token`].
    pub(crate) async fn person_github_token(
        &self,
        user: &UserRecord,
        action: &str,
    ) -> AppResult<Secret> {
        if user.github_login.is_none() {
            return Err(AppError::Forbidden(format!(
                "{action} acts as a person on GitHub, and the superadmin account has no GitHub \
                 identity. Do it on GitHub, or sign in with GitHub as an admitted user."
            )));
        }
        self.user_github_token(user).await
    }

    /// The GitHub token of the person who owns `task` (SPEC-070; its creator until a hand-over),
    /// for what the task does on GitHub (F 2026-10-11): Yulius's pushes and pull requests for it,
    /// its agent's Git and `gh`, and the agent's delivery tools, so GitHub shows that person as
    /// the actor. `None` when the owner has no usable token — no linked GitHub account (the
    /// superadmin, ACC-012), a disabled or deleted account, or a sign-in that can no longer be
    /// refreshed. The caller then acts as the GitHub App, and the task's terminal says so once
    /// (again after a hand-over).
    pub(crate) async fn owner_github_token(&self, task: &TaskRecord) -> Option<Secret> {
        let reason = match self.deps.store.user(&task.owner).await {
            Ok(Some(user)) if user.disabled_at.is_some() => "its owner's account is disabled",
            Ok(Some(user)) if user.github_login.is_none() => {
                "its owner has no linked GitHub account"
            }
            Ok(Some(user)) => match self.user_github_token(&user).await {
                Ok(token) => return Some(token),
                Err(err) => {
                    tracing::info!(task = %task.id, error = %err, "owner's GitHub token unusable");
                    "its owner's GitHub sign-in has expired (signing in with GitHub renews it)"
                }
            },
            Ok(None) => "its owner's account no longer exists",
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "reading the task owner failed");
                "its owner's account could not be read"
            }
        };
        let runtime = self.runtime(&task.id);
        if !runtime.acting_as_app_said.swap(true, Ordering::SeqCst) {
            runtime.hub.notice(format!(
                "This task acts on GitHub as the Yulius GitHub App, not as a person: {reason}."
            ));
        }
        None
    }

    /// [`App::owner_github_token`], else the App's installation token for `sandbox`'s owner:
    /// the token for Yulius's own GitHub calls for a task (pushes, pull requests, merges,
    /// promotions).
    ///
    /// # Errors
    /// Without a usable owner token: the App is not installed; GitHub failures.
    pub(crate) async fn task_github_token(
        &self,
        task: &TaskRecord,
        sandbox: &SandboxRecord,
    ) -> AppResult<Secret> {
        match self.owner_github_token(task).await {
            Some(token) => Ok(token),
            None => self.dlv_installation_token(sandbox).await,
        }
    }

    /// Requires `action` on `sandbox`; invisible sandboxes yield `NotFound`.
    ///
    /// # Errors
    /// `NotFound` without access, `Forbidden` with insufficient access.
    pub async fn require(
        &self,
        user: &UserRecord,
        sandbox_id: &SandboxId,
        action: Action,
    ) -> AppResult<(SandboxRecord, AccessLevel)> {
        let sandbox = self
            .deps
            .store
            .sandbox(sandbox_id)
            .await?
            .filter(|s| s.removed_at.is_none())
            .ok_or(AppError::NotFound)?;
        let level = self.access_level(user, &sandbox).await?;
        if level == AccessLevel::None {
            return Err(AppError::NotFound);
        }
        if !level.allows(action) {
            return Err(AppError::Forbidden(format!(
                "this action needs {} access on GitHub; you have {}",
                action.required_level().as_str(),
                level.as_str()
            )));
        }
        Ok((sandbox, level))
    }

    /// Whether the user may view a sandbox (errors count as no).
    pub async fn can_view(&self, user: &UserRecord, sandbox: &SandboxId) -> bool {
        self.require(user, sandbox, Action::View).await.is_ok()
    }

    /// Sandboxes the user can see with their levels. GitHub failures for one sandbox hide it
    /// rather than failing the whole list.
    ///
    /// # Errors
    /// Store failures.
    pub async fn visible_sandboxes(
        &self,
        user: &UserRecord,
    ) -> AppResult<Vec<(SandboxRecord, AccessLevel)>> {
        let mut out = Vec::new();
        for sandbox in self.deps.store.sandboxes().await? {
            match self.access_level(user, &sandbox).await {
                Ok(AccessLevel::None) => {}
                Ok(level) => out.push((sandbox, level)),
                Err(err) => {
                    tracing::warn!(error = %err, sandbox = %sandbox.slug, "permission lookup failed")
                }
            }
        }
        Ok(out)
    }
}
