//! Superadmin GitHub connection with GitHub's App manifest flow (SPEC-001 ACC-006, journey J1
//! step 3; SPEC-002 ADR-08).
//!
//! 1. `github_manifest` returns the manifest and GitHub's form URL with a one-time `state`.
//! 2. The browser posts the manifest to GitHub; the user confirms the App there.
//! 3. GitHub redirects to `/api/admin/github/manifest/callback?code&state`;
//!    `github_manifest_callback` converts the code into App credentials, seals them and
//!    returns the App's installation URL.

use crate::auth::{AuthContext, hex_token};
use crate::context::App;
use crate::error::{AppError, AppResult};

/// Lifetime of a manifest `state` (the user confirms the App on GitHub meanwhile).
const MANIFEST_STATE_TTL_MS: i64 = 30 * 60 * 1000;
/// `next_path` marker of manifest states (sign-in states never carry it).
const MANIFEST_STATE_PREFIX: &str = "manifest:";

/// The instance's GitHub connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubAdminView {
    /// An App is configured.
    pub configured: bool,
    /// App slug.
    pub app_slug: Option<String>,
    /// Owner login.
    pub owner_login: Option<String>,
    /// `org` or `user`.
    pub owner_kind: Option<String>,
    /// Where to install the App on repositories.
    pub install_url: Option<String>,
    /// Installation id once known.
    pub installation_id: Option<u64>,
}

fn install_url(slug: &str) -> String {
    format!("https://github.com/apps/{slug}/installations/new")
}

fn valid_owner(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 39
        && !login.starts_with('-')
        && login
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

impl App {
    /// The GitHub connection state (superadmin).
    ///
    /// # Errors
    /// `Forbidden` for non-superadmins; store failures.
    pub async fn github_admin(&self, ctx: &AuthContext) -> AppResult<GitHubAdminView> {
        self.require_superadmin(ctx)?;
        let instance = self.deps.store.instance().await?;
        let (owner_login, owner_kind, installation_id) = instance
            .as_ref()
            .map(|i| {
                (
                    i.owner_login.clone(),
                    i.owner_kind.clone(),
                    i.installation_id,
                )
            })
            .unwrap_or_default();
        match self.github_app().await {
            Ok(app) => Ok(GitHubAdminView {
                configured: true,
                install_url: Some(install_url(&app.slug)),
                app_slug: Some(app.slug),
                owner_login,
                owner_kind,
                installation_id,
            }),
            Err(AppError::Conflict(_)) => Ok(GitHubAdminView {
                configured: false,
                app_slug: None,
                owner_login,
                owner_kind,
                install_url: None,
                installation_id: None,
            }),
            Err(err) => Err(err),
        }
    }

    /// Prepares the App manifest for `owner_login` (`owner_kind` `org` or `user`): returns
    /// GitHub's form URL (with a one-time state) and the manifest JSON.
    ///
    /// # Errors
    /// `Forbidden` for non-superadmins, `Invalid` input or a non-https public URL, `Conflict`
    /// when an App is already configured.
    pub async fn github_manifest(
        &self,
        ctx: &AuthContext,
        owner_login: &str,
        owner_kind: &str,
    ) -> AppResult<(String, serde_json::Value)> {
        self.require_superadmin(ctx)?;
        if !valid_owner(owner_login) {
            return Err(AppError::Invalid(
                "enter a GitHub organization or user name".into(),
            ));
        }
        let is_org = match owner_kind {
            "org" => true,
            "user" => false,
            _ => return Err(AppError::Invalid("owner_kind must be org or user".into())),
        };
        if self.github_app().await.is_ok() {
            return Err(AppError::Conflict(
                "a GitHub App is already configured".into(),
            ));
        }
        let manifest = self
            .deps
            .github
            .app_manifest(&self.config.public_url, owner_login);
        if manifest.is_null() {
            return Err(AppError::Invalid(
                "the public URL must use https (or http on localhost) for the GitHub App".into(),
            ));
        }
        let state = hex_token(&self.deps.crypto.random_bytes());
        let now = self.now();
        self.deps
            .store
            .insert_oauth_state(
                &self.deps.crypto.sha256_hex(state.as_bytes()),
                &format!("{MANIFEST_STATE_PREFIX}{owner_kind}:{owner_login}"),
                now,
                now.plus_millis(MANIFEST_STATE_TTL_MS),
            )
            .await?;
        let post_url = self
            .deps
            .github
            .manifest_post_url_with_state(owner_login, is_org, &state);
        Ok((post_url, manifest))
    }

    /// Completes the manifest flow: converts GitHub's `code` into App credentials, stores them
    /// sealed with the owner and returns the App's installation URL.
    ///
    /// # Errors
    /// `Forbidden` for non-superadmins or an unknown/expired state; GitHub failures.
    pub async fn github_manifest_callback(
        &self,
        ctx: &AuthContext,
        code: &str,
        state: &str,
    ) -> AppResult<String> {
        self.require_superadmin(ctx)?;
        if code.is_empty() || code.len() > 512 || state.is_empty() || state.len() > 512 {
            return Err(AppError::Invalid("missing code or state".into()));
        }
        let next = self
            .deps
            .store
            .take_oauth_state(&self.deps.crypto.sha256_hex(state.as_bytes()), self.now())
            .await?
            .ok_or_else(|| {
                AppError::Forbidden("the GitHub App setup expired; start again".into())
            })?;
        let rest = next
            .strip_prefix(MANIFEST_STATE_PREFIX)
            .ok_or_else(|| AppError::Forbidden("not a GitHub App setup".into()))?;
        let (kind, owner) = rest
            .split_once(':')
            .ok_or_else(|| AppError::Forbidden("not a GitHub App setup".into()))?;
        let app = self.deps.github.convert_manifest(code).await?;
        let sealed = self
            .deps
            .crypto
            .seal(&serde_json::to_vec(&app).map_err(AppError::internal)?)?;
        self.deps.store.set_github_app(&sealed, owner, kind).await?;
        self.deps
            .store
            .audit(
                self.now(),
                &self.login_of(&ctx.user.id).await,
                "github.app_configured",
                None,
                Some(&format!("{} for {kind} {owner}", app.slug)),
            )
            .await?;
        Ok(install_url(&app.slug))
    }
}

#[cfg(test)]
mod tests {
    use super::valid_owner;

    #[test]
    fn owner_names_follow_github_rules() {
        assert!(valid_owner("Quartio-Studio"));
        assert!(!valid_owner(""));
        assert!(!valid_owner("-bad"));
        assert!(!valid_owner("a/b"));
        assert!(!valid_owner(&"x".repeat(40)));
    }
}
