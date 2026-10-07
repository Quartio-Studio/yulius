//! GitHub port (SPEC-002 ADR-08). Implemented over `yulius-github` at composition.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use yulius_domain::permission::GitHubPermissions;
use yulius_domain::time::Timestamp;

use crate::error::AppResult;
use crate::model::Secret;

/// GitHub App credentials (sealed as JSON in the instance row).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHubAppConfig {
    /// Numeric App id.
    pub app_id: u64,
    /// App slug (for install URLs).
    pub slug: String,
    /// OAuth client id.
    pub client_id: String,
    /// OAuth client secret.
    pub client_secret: String,
    /// PEM private key for App JWTs.
    pub private_key_pem: String,
    /// Webhook secret.
    pub webhook_secret: Option<String>,
}

impl std::fmt::Debug for GitHubAppConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitHubAppConfig")
            .field("app_id", &self.app_id)
            .field("slug", &self.slug)
            .finish_non_exhaustive()
    }
}

/// A GitHub user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhUser {
    /// Numeric id.
    pub id: u64,
    /// Login.
    pub login: String,
    /// Display name.
    pub name: Option<String>,
    /// Avatar URL.
    pub avatar_url: Option<String>,
}

/// A user-to-server token pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhUserToken {
    /// Access token.
    pub access: Secret,
    /// Refresh token.
    pub refresh: Option<Secret>,
    /// Access expiry.
    pub expires_at: Option<Timestamp>,
    /// Refresh expiry.
    pub refresh_expires_at: Option<Timestamp>,
}

/// A repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhRepo {
    /// Numeric id.
    pub id: u64,
    /// Owner login.
    pub owner: String,
    /// Name.
    pub name: String,
    /// Private.
    pub private: bool,
    /// Default branch.
    pub default_branch: String,
    /// Browser URL.
    pub html_url: String,
    /// HTTPS clone URL.
    pub clone_url: String,
}

/// Result of a permission lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoAccess {
    /// The user cannot see the repository.
    NoAccess,
    /// The user's permissions.
    Access {
        /// Permission flags.
        permissions: GitHubPermissions,
        /// Repository facts.
        repo: GhRepo,
    },
}

/// Branch protection request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulesetRequest {
    /// Ruleset name.
    pub name: String,
    /// Protected branch.
    pub branch: String,
    /// Require pull requests.
    pub require_pr: bool,
    /// Allowed merge methods (`squash`, `merge`, `rebase`).
    pub merge_methods: Vec<String>,
}

/// Aggregated checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksState {
    /// No checks reported.
    None,
    /// Some checks pending.
    Pending,
    /// All passed.
    Success,
    /// Any failed.
    Failure,
}

impl ChecksState {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Failure => "failure",
        }
    }
}

/// A pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhPull {
    /// Number.
    pub number: u64,
    /// URL.
    pub html_url: String,
    /// Title.
    pub title: String,
    /// `open`, `closed`.
    pub state: String,
    /// Merged flag.
    pub merged: bool,
    /// Base branch.
    pub base: String,
    /// Head branch.
    pub head: String,
    /// Head SHA.
    pub head_sha: String,
}

/// A pull request to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPull {
    /// Head branch.
    pub head: String,
    /// Base branch.
    pub base: String,
    /// Title.
    pub title: String,
    /// Body.
    pub body: String,
}

/// GitHub operations used by the application.
#[async_trait]
pub trait GitHubPort: Send + Sync {
    /// OAuth authorization URL for user sign-in.
    fn authorize_url(&self, app: &GitHubAppConfig, state: &str, redirect_uri: &str) -> String;
    /// Exchanges an OAuth code.
    async fn exchange_code(
        &self,
        app: &GitHubAppConfig,
        code: &str,
        redirect_uri: &str,
    ) -> AppResult<GhUserToken>;
    /// Refreshes a user token.
    async fn refresh_token(
        &self,
        app: &GitHubAppConfig,
        refresh: &Secret,
    ) -> AppResult<GhUserToken>;
    /// The token's user.
    async fn current_user(&self, token: &Secret) -> AppResult<GhUser>;
    /// The user's permissions on a repository.
    async fn repo_access(&self, token: &Secret, owner: &str, name: &str) -> AppResult<RepoAccess>;
    /// Installation id of the App on an owner.
    async fn find_installation(&self, app: &GitHubAppConfig, owner: &str)
    -> AppResult<Option<u64>>;
    /// Short-lived installation token.
    async fn installation_token(
        &self,
        app: &GitHubAppConfig,
        installation_id: u64,
    ) -> AppResult<Secret>;

    /// Installation token restricted to the repository `repo` of the installation's owner
    /// (for agents). The default falls back to an unrestricted installation token.
    ///
    /// # Errors
    /// GitHub failures.
    async fn repository_token(
        &self,
        app: &GitHubAppConfig,
        installation_id: u64,
        repo: &str,
    ) -> AppResult<Secret> {
        let _ = repo;
        self.installation_token(app, installation_id).await
    }
    /// Repositories the installation can access.
    async fn installation_repos(
        &self,
        app: &GitHubAppConfig,
        installation_id: u64,
    ) -> AppResult<Vec<GhRepo>>;
    /// Creates a private repository (org: installation token; user: owner's user token).
    async fn create_repo(
        &self,
        token: &Secret,
        owner: &str,
        owner_is_org: bool,
        name: &str,
        description: &str,
    ) -> AppResult<GhRepo>;
    /// Creates a branch ruleset.
    async fn create_ruleset(
        &self,
        token: &Secret,
        owner: &str,
        name: &str,
        request: &RulesetRequest,
    ) -> AppResult<()>;
    /// App manifest for the creation flow.
    fn app_manifest(&self, public_url: &str, owner_login: &str) -> serde_json::Value;
    /// URL the manifest form is posted to.
    fn manifest_post_url(&self, owner_login: &str, owner_is_org: bool) -> String;
    /// [`Self::manifest_post_url`] with a one-time `state` that GitHub returns to the callback.
    fn manifest_post_url_with_state(
        &self,
        owner_login: &str,
        owner_is_org: bool,
        state: &str,
    ) -> String {
        let base = self.manifest_post_url(owner_login, owner_is_org);
        let sep = if base.contains('?') { '&' } else { '?' };
        format!("{base}{sep}state={state}")
    }
    /// Converts a manifest code into App credentials.
    async fn convert_manifest(&self, code: &str) -> AppResult<GitHubAppConfig>;
    /// Opens a pull request.
    async fn create_pull(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        pull: &NewPull,
    ) -> AppResult<GhPull>;
    /// Gets a pull request.
    async fn get_pull(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> AppResult<GhPull>;
    /// Open pull requests (optionally for a head branch).
    async fn list_pulls(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        head: Option<&str>,
    ) -> AppResult<Vec<GhPull>>;
    /// Aggregated checks of a commit.
    async fn checks(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> AppResult<ChecksState>;
    /// Merges a pull request at the expected head.
    async fn merge_pull(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        number: u64,
        method: &str,
        expected_head: &str,
    ) -> AppResult<String>;
    /// Fast-forwards `branch` to `sha` (refuses non-fast-forward).
    async fn fast_forward(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        branch: &str,
        sha: &str,
    ) -> AppResult<()>;
    /// Head SHA of a branch.
    async fn branch_sha(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> AppResult<Option<String>>;
}
