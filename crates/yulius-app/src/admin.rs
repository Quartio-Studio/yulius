//! Superadmin user administration (SPEC-001 ACC-004, ACC-009).

use yulius_contracts::api::admin::{AdminUser, AdmitUser, InstanceSettings, UpdateUser};
use yulius_domain::ids::UserId;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::UserRecord;

fn view(user: &UserRecord) -> AdminUser {
    AdminUser {
        id: user.id.to_string(),
        github_login: user.github_login.clone().unwrap_or_default(),
        display_name: user.display_name.clone(),
        is_instance_admin: user.is_instance_admin,
        disabled: user.disabled_at.is_some(),
        github_linked: user.github_id.is_some(),
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
    /// Lists users except the superadmin.
    ///
    /// # Errors
    /// `Forbidden` for non-superadmins.
    pub async fn admin_users(&self, ctx: &AuthContext) -> AppResult<Vec<AdminUser>> {
        self.require_superadmin(ctx)?;
        Ok(self
            .deps
            .store
            .users()
            .await?
            .iter()
            .filter(|u| !u.is_superadmin)
            .map(view)
            .collect())
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
        Ok(view(&user))
    }

    /// Disables/enables a user or changes the admin flag. Disabling revokes sessions and tokens.
    ///
    /// # Errors
    /// `NotFound`, `Forbidden`.
    pub async fn update_user(
        &self,
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
        let updated = store.user(&id).await?.ok_or(AppError::NotFound)?;
        Ok(view(&updated))
    }
}

impl App {
    /// Instance behavior settings (instance administrators).
    ///
    /// # Errors
    /// `Forbidden` for other users.
    pub async fn instance_settings(&self, ctx: &AuthContext) -> AppResult<InstanceSettings> {
        self.require_instance_admin(ctx)?;
        let instance = self
            .deps
            .store
            .instance()
            .await?
            .ok_or(AppError::NotFound)?;
        Ok(InstanceSettings {
            auto_harness_switch: instance.auto_harness_switch,
        })
    }

    /// Updates instance behavior settings (audited).
    ///
    /// # Errors
    /// `Forbidden` for other users.
    pub async fn update_instance_settings(
        &self,
        ctx: &AuthContext,
        settings: &InstanceSettings,
    ) -> AppResult<InstanceSettings> {
        self.require_instance_admin(ctx)?;
        self.deps
            .store
            .set_auto_harness_switch(settings.auto_harness_switch)
            .await?;
        let actor = ctx
            .user
            .github_login
            .clone()
            .unwrap_or_else(|| "superadmin".into());
        let _ = self
            .deps
            .store
            .audit(
                self.now(),
                &actor,
                "settings.updated",
                None,
                Some(&format!(
                    "auto_harness_switch={}",
                    settings.auto_harness_switch
                )),
            )
            .await;
        self.instance_settings(ctx).await
    }
}

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
