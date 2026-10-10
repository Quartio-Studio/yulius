//! [`GitHubPort`] over `yulius-github` (App JWT, installation tokens, OAuth, REST).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::Secret;
use yulius_app::ports::github::{
    ChecksReport, ChecksSource, ChecksState, GhPull, GhRepo, GhUser, GhUserToken, GitHubAppConfig,
    GitHubPort, NewPull, RepoAccess, RulesetRequest,
};
use yulius_github::{
    AppCredentials, CheckSource, ChecksSummary, CreateRepo, GitHubClient, GitHubConfig,
    GitHubError, MergeMethod, OwnerKind, PullFilter, PullRequest, PullState, Repo, RulesetSpec,
    Secret as GhSecret, Url,
};

/// GitHub adapter with one cached client per App id (keeps installation tokens warm).
pub struct GitHubAdapter {
    base: GitHubConfig,
    cached: Mutex<Option<(u64, Arc<GitHubClient>)>>,
    plain: Arc<GitHubClient>,
}

impl GitHubAdapter {
    /// Adapter for github.com (or `base` overrides, e.g. a test server).
    ///
    /// # Errors
    /// Fails when the HTTP client cannot be built.
    pub fn new(base: GitHubConfig) -> Result<Self, String> {
        let plain = GitHubClient::new(base.clone()).map_err(|e| e.to_string())?;
        Ok(Self {
            base,
            cached: Mutex::new(None),
            plain: Arc::new(plain),
        })
    }

    fn client(&self, app: &GitHubAppConfig) -> AppResult<Arc<GitHubClient>> {
        let mut cached = self
            .cached
            .lock()
            .map_err(|_| AppError::Internal("github client cache poisoned".into()))?;
        if let Some((id, client)) = cached.as_ref()
            && *id == app.app_id
        {
            return Ok(client.clone());
        }
        let mut config = self.base.clone();
        config.app = Some(AppCredentials {
            app_id: app.app_id,
            client_id: app.client_id.clone(),
            client_secret: GhSecret::new(app.client_secret.clone()),
            private_key_pem: GhSecret::new(app.private_key_pem.clone()),
            slug: app.slug.clone(),
            webhook_secret: app.webhook_secret.clone().map(GhSecret::new),
        });
        let client = Arc::new(GitHubClient::new(config).map_err(map_err)?);
        *cached = Some((app.app_id, client.clone()));
        Ok(client)
    }
}

fn map_err(err: GitHubError) -> AppError {
    match err {
        GitHubError::Unauthorized => AppError::Unauthenticated,
        GitHubError::NotFound => AppError::NotFound,
        GitHubError::Conflict { .. }
        | GitHubError::NotFastForward
        | GitHubError::NotMergeable { .. } => AppError::Conflict(err.to_string()),
        other => AppError::Upstream(format!("GitHub: {other}")),
    }
}

fn gh(secret: &Secret) -> GhSecret {
    GhSecret::new(secret.expose().to_owned())
}

fn app_secret(secret: &GhSecret) -> Secret {
    Secret(secret.expose().to_owned())
}

fn repo(r: Repo) -> GhRepo {
    GhRepo {
        id: r.id,
        owner: r.owner,
        name: r.name,
        private: r.private,
        default_branch: r.default_branch,
        html_url: r.html_url,
        clone_url: r.clone_url,
    }
}

fn pull(p: PullRequest) -> GhPull {
    GhPull {
        number: p.number,
        html_url: p.url,
        title: p.title,
        state: match p.state {
            PullState::Open => "open".into(),
            PullState::Closed | PullState::Merged => "closed".into(),
        },
        merged: p.state == PullState::Merged,
        base: p.base,
        head: p.head,
        head_sha: p.head_sha,
    }
}

fn merge_method(text: &str) -> AppResult<MergeMethod> {
    match text {
        "merge" | "merge_commit" => Ok(MergeMethod::Merge),
        "squash" => Ok(MergeMethod::Squash),
        "rebase" => Ok(MergeMethod::Rebase),
        other => Err(AppError::Invalid(format!("unknown merge method {other}"))),
    }
}

#[async_trait]
impl GitHubPort for GitHubAdapter {
    fn authorize_url(
        &self,
        app: &GitHubAppConfig,
        state: &str,
        redirect_uri: &str,
        select_account: bool,
    ) -> String {
        self.client(app)
            .and_then(|c| {
                c.authorize_url(state, redirect_uri, select_account)
                    .map_err(map_err)
            })
            .unwrap_or_else(|_| "about:blank".into())
    }

    async fn exchange_code(
        &self,
        app: &GitHubAppConfig,
        code: &str,
        redirect_uri: &str,
    ) -> AppResult<GhUserToken> {
        let t = self
            .client(app)?
            .exchange_code(code, redirect_uri)
            .await
            .map_err(map_err)?;
        Ok(GhUserToken {
            access: app_secret(&t.access),
            refresh: t.refresh.as_ref().map(app_secret),
            expires_at: t.expires_at,
            refresh_expires_at: t.refresh_expires_at,
        })
    }

    async fn refresh_token(
        &self,
        app: &GitHubAppConfig,
        refresh: &Secret,
    ) -> AppResult<GhUserToken> {
        let t = self
            .client(app)?
            .refresh_user_token(&gh(refresh))
            .await
            .map_err(map_err)?;
        Ok(GhUserToken {
            access: app_secret(&t.access),
            refresh: t.refresh.as_ref().map(app_secret),
            expires_at: t.expires_at,
            refresh_expires_at: t.refresh_expires_at,
        })
    }

    async fn current_user(&self, token: &Secret) -> AppResult<GhUser> {
        let u = self.plain.current_user(&gh(token)).await.map_err(map_err)?;
        Ok(GhUser {
            id: u.id,
            login: u.login,
            name: u.name,
            avatar_url: u.avatar_url,
        })
    }

    async fn repo_access(&self, token: &Secret, owner: &str, name: &str) -> AppResult<RepoAccess> {
        match self
            .plain
            .repo_permissions(&gh(token), owner, name)
            .await
            .map_err(map_err)?
        {
            yulius_github::RepoAccess::NoAccess => Ok(RepoAccess::NoAccess),
            yulius_github::RepoAccess::Granted {
                permissions,
                repo: r,
            } => Ok(RepoAccess::Access {
                permissions,
                repo: repo(r),
            }),
        }
    }

    async fn find_installation(
        &self,
        app: &GitHubAppConfig,
        owner: &str,
    ) -> AppResult<Option<u64>> {
        self.client(app)?
            .find_installation(owner)
            .await
            .map_err(map_err)
    }

    async fn installation_token(
        &self,
        app: &GitHubAppConfig,
        installation_id: u64,
    ) -> AppResult<Secret> {
        let t = self
            .client(app)?
            .create_installation_token(installation_id)
            .await
            .map_err(map_err)?;
        Ok(app_secret(&t.token))
    }

    async fn repository_token(
        &self,
        app: &GitHubAppConfig,
        installation_id: u64,
        repo: &str,
    ) -> AppResult<Secret> {
        let t = self
            .client(app)?
            .create_repository_token(installation_id, repo)
            .await
            .map_err(map_err)?;
        Ok(app_secret(&t.token))
    }

    async fn installation_repos(
        &self,
        app: &GitHubAppConfig,
        installation_id: u64,
    ) -> AppResult<Vec<GhRepo>> {
        Ok(self
            .client(app)?
            .installation_repositories(installation_id)
            .await
            .map_err(map_err)?
            .into_iter()
            .map(repo)
            .collect())
    }

    async fn create_repo(
        &self,
        token: &Secret,
        owner: &str,
        owner_is_org: bool,
        name: &str,
        description: &str,
    ) -> AppResult<GhRepo> {
        let mut spec = CreateRepo::private(name);
        spec.description = (!description.is_empty()).then(|| description.to_owned());
        let created = if owner_is_org {
            self.plain.create_org_repo(&gh(token), owner, &spec).await
        } else {
            self.plain.create_user_repo(&gh(token), &spec).await
        };
        Ok(repo(created.map_err(map_err)?))
    }

    async fn create_ruleset(
        &self,
        token: &Secret,
        owner: &str,
        name: &str,
        request: &RulesetRequest,
    ) -> AppResult<()> {
        let methods = request
            .merge_methods
            .iter()
            .map(|m| merge_method(m))
            .collect::<AppResult<Vec<_>>>()?;
        let spec = RulesetSpec {
            name: request.name.clone(),
            branch: request.branch.clone(),
            require_pr: request.require_pr,
            allowed_merge_methods: methods,
            block_force_push: true,
            block_deletion: true,
        };
        self.plain
            .create_branch_ruleset(&gh(token), owner, name, &spec)
            .await
            .map(|_| ())
            .map_err(map_err)
    }

    fn app_manifest(&self, public_url: &str, owner_login: &str) -> serde_json::Value {
        let Ok(url) = Url::parse(public_url) else {
            return serde_json::Value::Null;
        };
        self.plain
            .app_manifest(&url, owner_login, OwnerKind::Organization)
            .unwrap_or(serde_json::Value::Null)
    }

    fn manifest_post_url(&self, owner_login: &str, owner_is_org: bool) -> String {
        let kind = if owner_is_org {
            OwnerKind::Organization
        } else {
            OwnerKind::User
        };
        self.plain
            .manifest_post_url(owner_login, kind)
            .unwrap_or_else(|_| "about:blank".into())
    }

    fn manifest_post_url_with_state(
        &self,
        owner_login: &str,
        owner_is_org: bool,
        state: &str,
    ) -> String {
        let kind = if owner_is_org {
            OwnerKind::Organization
        } else {
            OwnerKind::User
        };
        self.plain
            .manifest_post_url_with_state(owner_login, kind, state)
            .unwrap_or_else(|_| "about:blank".into())
    }

    async fn convert_manifest(&self, code: &str) -> AppResult<GitHubAppConfig> {
        let c = self.plain.convert_manifest(code).await.map_err(map_err)?;
        Ok(GitHubAppConfig {
            app_id: c.app_id,
            slug: c.slug,
            client_id: c.client_id,
            client_secret: c.client_secret.expose().to_owned(),
            private_key_pem: c.private_key_pem.expose().to_owned(),
            webhook_secret: c.webhook_secret.map(|s| s.expose().to_owned()),
        })
    }

    async fn create_pull(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        new: &NewPull,
    ) -> AppResult<GhPull> {
        let spec = yulius_github::NewPull {
            head: new.head.clone(),
            base: new.base.clone(),
            title: new.title.clone(),
            body: new.body.clone(),
        };
        Ok(pull(
            self.plain
                .create_pull(&gh(token), owner, repo_name, &spec)
                .await
                .map_err(map_err)?,
        ))
    }

    async fn get_pull(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        number: u64,
    ) -> AppResult<GhPull> {
        Ok(pull(
            self.plain
                .get_pull(&gh(token), owner, repo_name, number)
                .await
                .map_err(map_err)?,
        ))
    }

    async fn list_pulls(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        head: Option<&str>,
    ) -> AppResult<Vec<GhPull>> {
        Ok(self
            .plain
            .list_pulls(&gh(token), owner, repo_name, PullFilter::Open, head)
            .await
            .map_err(map_err)?
            .into_iter()
            .map(pull)
            .collect())
    }

    async fn checks(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        sha: &str,
    ) -> AppResult<ChecksReport> {
        let report = self
            .plain
            .check_status(&gh(token), owner, repo_name, sha)
            .await
            .map_err(map_err)?;
        Ok(ChecksReport {
            state: match report.summary {
                ChecksSummary::None => ChecksState::None,
                ChecksSummary::Pending => ChecksState::Pending,
                ChecksSummary::Success => ChecksState::Success,
                ChecksSummary::Failure => ChecksState::Failure,
                ChecksSummary::Unknown => ChecksState::Unknown,
            },
            unreadable: report
                .unreadable
                .into_iter()
                .map(|source| match source {
                    CheckSource::CheckRuns => ChecksSource::CheckRuns,
                    CheckSource::CommitStatuses => ChecksSource::CommitStatuses,
                    CheckSource::WorkflowRuns => ChecksSource::WorkflowRuns,
                })
                .collect(),
        })
    }

    async fn merge_pull(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        number: u64,
        method: &str,
        expected_head: &str,
    ) -> AppResult<String> {
        let outcome = self
            .plain
            .merge_pull(
                &gh(token),
                owner,
                repo_name,
                number,
                merge_method(method)?,
                expected_head,
            )
            .await
            .map_err(map_err)?;
        Ok(outcome.sha)
    }

    async fn fast_forward(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        branch: &str,
        sha: &str,
    ) -> AppResult<()> {
        self.plain
            .fast_forward_ref(&gh(token), owner, repo_name, branch, sha)
            .await
            .map_err(map_err)
    }

    async fn branch_sha(
        &self,
        _token: &Secret,
        _owner: &str,
        _repo: &str,
        _branch: &str,
    ) -> AppResult<Option<String>> {
        Err(AppError::Upstream(
            "branch lookup via GitHub is not supported; read the fetched clone instead".into(),
        ))
    }
}
