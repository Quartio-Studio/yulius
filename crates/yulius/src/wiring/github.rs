//! GitHub port placeholder until the GitHub adapter is wired (CP4). Every call fails with a
//! clear upstream error; nothing pretends to succeed.

use async_trait::async_trait;
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::Secret;
use yulius_app::ports::github::{
    ChecksState, GhPull, GhRepo, GhUser, GhUserToken, GitHubAppConfig, GitHubPort, NewPull,
    RepoAccess, RulesetRequest,
};

/// GitHub port that reports the integration as unavailable.
pub struct UnavailableGitHub;

fn unavailable<T>() -> AppResult<T> {
    Err(AppError::Upstream(
        "GitHub integration is not available in this build".into(),
    ))
}

#[async_trait]
impl GitHubPort for UnavailableGitHub {
    fn authorize_url(&self, _: &GitHubAppConfig, _: &str, _: &str, _: bool) -> String {
        String::from("about:blank")
    }
    async fn exchange_code(&self, _: &GitHubAppConfig, _: &str, _: &str) -> AppResult<GhUserToken> {
        unavailable()
    }
    async fn refresh_token(&self, _: &GitHubAppConfig, _: &Secret) -> AppResult<GhUserToken> {
        unavailable()
    }
    async fn current_user(&self, _: &Secret) -> AppResult<GhUser> {
        unavailable()
    }
    async fn repo_access(&self, _: &Secret, _: &str, _: &str) -> AppResult<RepoAccess> {
        unavailable()
    }
    async fn find_installation(&self, _: &GitHubAppConfig, _: &str) -> AppResult<Option<u64>> {
        unavailable()
    }
    async fn installation_token(&self, _: &GitHubAppConfig, _: u64) -> AppResult<Secret> {
        unavailable()
    }
    async fn installation_repos(&self, _: &GitHubAppConfig, _: u64) -> AppResult<Vec<GhRepo>> {
        unavailable()
    }
    async fn create_repo(
        &self,
        _: &Secret,
        _: &str,
        _: bool,
        _: &str,
        _: &str,
    ) -> AppResult<GhRepo> {
        unavailable()
    }
    async fn create_ruleset(
        &self,
        _: &Secret,
        _: &str,
        _: &str,
        _: &RulesetRequest,
    ) -> AppResult<()> {
        unavailable()
    }
    fn app_manifest(&self, _: &str, _: &str) -> serde_json::Value {
        serde_json::Value::Null
    }
    fn manifest_post_url(&self, _: &str, _: bool) -> String {
        String::from("about:blank")
    }
    async fn convert_manifest(&self, _: &str) -> AppResult<GitHubAppConfig> {
        unavailable()
    }
    async fn create_pull(&self, _: &Secret, _: &str, _: &str, _: &NewPull) -> AppResult<GhPull> {
        unavailable()
    }
    async fn get_pull(&self, _: &Secret, _: &str, _: &str, _: u64) -> AppResult<GhPull> {
        unavailable()
    }
    async fn list_pulls(
        &self,
        _: &Secret,
        _: &str,
        _: &str,
        _: Option<&str>,
    ) -> AppResult<Vec<GhPull>> {
        unavailable()
    }
    async fn checks(&self, _: &Secret, _: &str, _: &str, _: &str) -> AppResult<ChecksState> {
        unavailable()
    }
    async fn merge_pull(
        &self,
        _: &Secret,
        _: &str,
        _: &str,
        _: u64,
        _: &str,
        _: &str,
    ) -> AppResult<String> {
        unavailable()
    }
    async fn fast_forward(&self, _: &Secret, _: &str, _: &str, _: &str, _: &str) -> AppResult<()> {
        unavailable()
    }
    async fn branch_sha(&self, _: &Secret, _: &str, _: &str, _: &str) -> AppResult<Option<String>> {
        unavailable()
    }
}
