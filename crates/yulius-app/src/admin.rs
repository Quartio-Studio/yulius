//! User administration (SPEC-001 ACC-004, ACC-009): the superadmin admits and changes users;
//! instance administrators also read the list, with each person's two-factor (SPEC-061).

use std::sync::Arc;

use serde_json::json;
use yulius_contracts::api::admin::{AdminUser, AdmitUser, UpdateUser};
use yulius_domain::ids::UserId;
use yulius_domain::task::TaskStatus;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::UserRecord;

fn view(user: &UserRecord, two_factor: &str) -> AdminUser {
    AdminUser {
        id: user.id.to_string(),
        github_login: user.github_login.clone().unwrap_or_default(),
        display_name: user.display_name.clone(),
        is_instance_admin: user.is_instance_admin,
        disabled: user.disabled_at.is_some(),
        github_linked: user.github_id.is_some(),
        two_factor: two_factor.to_owned(),
    }
}

/// GitHub logins: 1–39 alphanumeric or single hyphens, not starting/ending with a hyphen.
fn valid_github_login(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 39
        && !login.starts_with('-')
        && !login.ends_with('-')
        && !login.contains("--")
        && login
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

impl App {
    /// Lists users except the superadmin and playground guests (whose playground's admins see and
    /// end them with their links, SPEC-410), with their two-factor (to reset a lost one).
    ///
    /// # Errors
    /// `Forbidden` for members.
    pub async fn admin_users(&self, ctx: &AuthContext) -> AppResult<Vec<AdminUser>> {
        self.require_instance_admin(ctx)?;
        let mut out = Vec::new();
        for user in self.deps.store.users().await? {
            if !user.is_superadmin && !self.is_guest(&user) {
                out.push(view(&user, self.two_factor_of(&user.id).await?));
            }
        }
        Ok(out)
    }

    /// Admits a GitHub user.
    ///
    /// # Errors
    /// `Invalid` login, `Conflict` duplicate, `Forbidden` for non-superadmins.
    pub async fn admit_user(&self, ctx: &AuthContext, req: &AdmitUser) -> AppResult<AdminUser> {
        self.require_superadmin(ctx)?;
        let login = req.github_login.trim();
        if !valid_github_login(login) {
            return Err(AppError::Invalid("not a valid GitHub login".into()));
        }
        let now = self.now();
        let user = UserRecord {
            id: UserId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?,
            github_login: Some(login.to_owned()),
            github_id: None,
            display_name: req
                .display_name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| login.to_owned()),
            avatar_url: None,
            is_superadmin: false,
            is_instance_admin: req.is_instance_admin,
            password_hash: None,
            created_at: now,
            disabled_at: None,
        };
        self.deps
            .store
            .insert_user(&user)
            .await
            .map_err(|e| match e {
                AppError::Conflict(_) => AppError::Conflict(format!("{login} is already admitted")),
                other => other,
            })?;
        self.deps
            .store
            .audit(now, "superadmin", "user.admitted", Some(login), None)
            .await?;
        Ok(view(&user, "missing"))
    }

    /// Disables/enables a user or changes the admin flag. Disabling revokes sessions and tokens
    /// and stops every task the person owns that is not resting (ADR-0481).
    ///
    /// # Errors
    /// `NotFound`, `Forbidden`.
    pub async fn update_user(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        req: &UpdateUser,
    ) -> AppResult<AdminUser> {
        self.require_superadmin(ctx)?;
        let id = UserId::parse(id).map_err(|_| AppError::NotFound)?;
        let store = &self.deps.store;
        let user = store
            .user(&id)
            .await?
            .filter(|u| !u.is_superadmin)
            .ok_or(AppError::NotFound)?;
        let now = self.now();
        let disabled = req.disabled.map(|d| d.then_some(now));
        store
            .update_user_flags(&id, disabled, req.is_instance_admin)
            .await?;
        if req.disabled == Some(true) {
            store.revoke_user_credentials(&id, now).await?;
            store.clear_access(&id).await?;
        }
        store
            .audit(
                now,
                "superadmin",
                "user.updated",
                user.github_login.as_deref(),
                Some(&format!("{req:?}")),
            )
            .await?;
        if req.disabled == Some(true) {
            // On a task of its own: a page that goes away does not leave the stops half done.
            let app = Arc::clone(self);
            let owner = user.id.clone();
            let stops = tokio::spawn(async move { app.stop_tasks_of_disabled(&owner).await });
            if let Err(err) = stops.await {
                tracing::warn!(error = %err, "stopping a disabled person's tasks failed");
            }
        }
        let updated = store.user(&id).await?.ok_or(AppError::NotFound)?;
        Ok(view(&updated, self.two_factor_of(&id).await?))
    }

    /// Stops every task `owner` owns that is not resting, live or about to start by itself
    /// (SPEC-675 L4Q3-002, ADR-0481): nothing started on a disabled person's authority runs on.
    /// Waiting tasks stop first, so a slot a stopped run frees starts none of them. A task is read
    /// again before its stop and left when someone took control meanwhile. Each stop is audited
    /// and said in the task's terminal; one that fails, or a failed listing, is logged and the
    /// rest go on (such a run is refused a git credential all the same).
    async fn stop_tasks_of_disabled(self: &Arc<Self>, owner: &UserId) {
        let store = &self.deps.store;
        let mut tasks = match store.tasks_with_status(&NOT_RESTING).await {
            Ok(tasks) => tasks,
            Err(err) => {
                tracing::warn!(error = %err, "a disabled person's tasks could not be listed");
                return;
            }
        };
        tasks.retain(|t| t.owner == *owner);
        tasks.sort_by_key(|t| t.status.is_live());
        let login = self.login_of(owner).await;
        for task in &tasks {
            match store.task(&task.id).await {
                Ok(Some(now)) if now.owner == *owner => {}
                Ok(_) => continue,
                Err(err) => {
                    tracing::warn!(task = %task.id, error = %err, "a disabled owner's task unreadable");
                    continue;
                }
            }
            if let Err(err) = self.stop_task_now(&task.id).await {
                tracing::warn!(task = %task.id, error = %err, "a disabled owner's task did not stop");
                continue;
            }
            let detail = json!({
                "task": format!("#{} {}", task.number, task.title),
                "owner": login,
                "reason": "owner disabled",
            });
            if let Err(err) = store
                .audit(
                    self.now(),
                    "superadmin",
                    "task.stopped",
                    Some(task.id.as_str()),
                    Some(&detail.to_string()),
                )
                .await
            {
                tracing::warn!(task = %task.id, error = %err, "a stop on disable was not audited");
            }
            if let Some(runtime) = self.existing_runtime(&task.id) {
                runtime
                    .hub
                    .notice(format!("Stopped: its owner @{login} was disabled."));
            }
        }
    }
}

/// Task statuses that hold a run or start one by themselves; [`TaskStatus::Stopping`] is on its
/// way to rest already.
const NOT_RESTING: [TaskStatus; 8] = [
    TaskStatus::Queued,
    TaskStatus::Starting,
    TaskStatus::Running,
    TaskStatus::Switching,
    TaskStatus::WaitingCapacity,
    TaskStatus::Chained,
    TaskStatus::Prepared,
    TaskStatus::Parked,
];

#[cfg(test)]
mod tests {
    use super::valid_github_login;

    #[test]
    fn github_login_rules() {
        assert!(valid_github_login("octo-cat"));
        assert!(valid_github_login("MrFishPL"));
        assert!(!valid_github_login("-bad"));
        assert!(!valid_github_login("bad-"));
        assert!(!valid_github_login("a--b"));
        assert!(!valid_github_login("has space"));
        assert!(!valid_github_login(""));
    }
}
