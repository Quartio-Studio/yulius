//! [`GitHubPort`] over `yulius-github` (App JWT, installation tokens, OAuth, REST).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::Secret;
use yulius_app::ports::github::{
    BranchGuard, ChecksReport, ChecksSource, ChecksState, GhIssue, GhPull, GhRepo, GhUser,
    GhUserToken, GitHubAppConfig, GitHubPort, NewCommitStatus, NewIssue, NewPull, RepoAccess,
    RoleGrant, RulesetRequest, STATUS_PERMISSION_MISSING,
};
use yulius_github::{
    AppCredentials, CheckSource, ChecksSummary, CollaboratorAdded, CommitStatus, CreateRepo,
    GitHubClient, GitHubConfig, GitHubError, MergeMethod, OwnerKind, PullFilter, PullRequest,
    PullState, Repo, RulesetSpec, Secret as GhSecret, Url,
};

/// GitHub adapter with one cached client per App id (keeps installation tokens warm).
pub struct GitHubAdapter {
    meter: yulius_github::RequestMeter,
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
        Self::metered(base, yulius_github::RequestMeter::default())
    }

    /// Use the instance meter shared with the update client.
    ///
    /// # Errors
    /// Fails when the HTTP client cannot be built.
    pub fn metered(base: GitHubConfig, meter: yulius_github::RequestMeter) -> Result<Self, String> {
        let plain = GitHubClient::metered(
            base.clone(),
            Arc::new(yulius_github::SystemClock),
            meter.clone(),
            yulius_app::ports::github::current_purpose,
        )
        .map_err(|e| e.to_string())?;
        Ok(Self {
            base,
            meter,
            cached: Mutex::new(None),
            plain: Arc::new(plain),
        })
    }

    fn for_token(&self, token: &Secret) -> Arc<GitHubClient> {
        if !token.is_github_installation() {
            return Arc::clone(&self.plain);
        }
        self.cached
            .lock()
            .ok()
            .and_then(|cached| cached.as_ref().map(|(_, client)| Arc::clone(client)))
            .unwrap_or_else(|| Arc::clone(&self.plain))
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
            webhook_secret: None,
        });
        let client = Arc::new(
            GitHubClient::metered(
                config,
                Arc::new(yulius_github::SystemClock),
                self.meter.clone(),
                yulius_app::ports::github::current_purpose,
            )
            .map_err(map_err)?,
        );
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
        GitHubError::RateLimited {
            retry_after: Some(wait),
        } => {
            let now = yulius_github::Clock::now(&yulius_github::SystemClock);
            let reset = yulius_domain::time::Timestamp(
                now.0
                    .saturating_add(i64::try_from(wait.as_millis()).unwrap_or(i64::MAX)),
            );
            AppError::Upstream(format!(
                "GitHub rate limit reached. Waiting until {}.",
                yulius_domain::time::format_rfc3339(reset)
            ))
        }
        other => AppError::Upstream(format!("GitHub: {other}")),
    }
}

/// [`map_err`] for a write whose refusal must not read as a failure that may pass: GitHub's
/// 4xx answers (a validation error, a missing permission) leave `Upstream`, which keeps a rate
/// limit, a 5xx, a timeout, the network and an unreadable answer (SPEC-069 BPL-008).
fn map_refusal(err: GitHubError) -> AppError {
    match err {
        GitHubError::Invalid { .. } | GitHubError::InvalidInput { .. } => {
            AppError::Invalid(format!("GitHub: {err}"))
        }
        GitHubError::Forbidden { .. } => AppError::Forbidden(format!("GitHub: {err}")),
        GitHubError::Upstream(status)
            if (400..500).contains(&status) && !matches!(status, 408 | 429) =>
        {
            AppError::Invalid(format!("GitHub: {err}"))
        }
        other => map_err(other),
    }
}

/// [`map_err`] for creating a repository: a 403 that names accepted permissions, or says
/// "Resource not accessible by integration", is a permission the App lacks and carries what
/// GitHub accepts (the `create_repo` port contract, SPEC-241 CRT-001); any other 403 (an
/// organization's policy) keeps GitHub's message.
fn map_creation(err: GitHubError) -> AppError {
    match err {
        GitHubError::Forbidden {
            message,
            accepted_permissions,
        } if accepted_permissions.is_some()
            || message
                .to_ascii_lowercase()
                .contains("not accessible by integration") =>
        {
            AppError::Forbidden(accepted_permissions.unwrap_or_default())
        }
        other => map_err(other),
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
        size_kib: r.size_kib,
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
        base_sha: p.base_sha,
        head: p.head,
        head_sha: p.head_sha,
        mergeable: p.mergeable,
        behind: p.behind,
        from_fork: p.cross_repository,
        merge_commit_sha: p.merge_commit_sha,
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
    fn budget(&self, installation: Option<u64>) -> yulius_app::ports::github::GitHubBudget {
        use yulius_github::Clock;
        self.meter
            .snapshot(installation, yulius_github::SystemClock.now())
    }

    fn rate_limit_reset(&self) -> Option<yulius_domain::time::Timestamp> {
        self.cached.lock().ok()?.as_ref()?.1.rate_limit_reset()
    }

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

    async fn callback_registered(
        &self,
        app: &GitHubAppConfig,
        redirect_uri: &str,
    ) -> AppResult<bool> {
        let told = self
            .client(app)?
            .callback_registered(redirect_uri)
            .await
            .map_err(|err| match err {
                // What a superadmin reads as the reason: GitHub's own code, and what it means
                // for the one a broken App gives.
                GitHubError::OAuth(code) if code == "incorrect_client_credentials" => {
                    AppError::Upstream(format!(
                        "GitHub answered `{code}`: it does not accept the App's client id and \
                         secret"
                    ))
                }
                GitHubError::OAuth(code) => AppError::Upstream(format!("GitHub answered `{code}`")),
                other => map_err(other),
            })?;
        told.ok_or_else(|| {
            AppError::Upstream(
                "GitHub does not say whether a callback URL is registered for this App: it \
                 takes one that cannot be registered too"
                    .into(),
            )
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
        let u = self
            .for_token(token)
            .current_user(&gh(token))
            .await
            .map_err(map_err)?;
        Ok(GhUser {
            id: u.id,
            login: u.login,
            name: u.name,
            avatar_url: u.avatar_url,
        })
    }

    async fn repo_access(&self, token: &Secret, owner: &str, name: &str) -> AppResult<RepoAccess> {
        match self
            .for_token(token)
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
        Ok(Secret::github_installation(t.token.expose().to_owned()))
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
        Ok(Secret::github_installation(t.token.expose().to_owned()))
    }

    async fn agent_token(
        &self,
        app: &GitHubAppConfig,
        installation_id: u64,
        repo: &str,
    ) -> AppResult<Secret> {
        let t = self
            .client(app)?
            .create_agent_token(installation_id, repo)
            .await
            .map_err(map_err)?;
        Ok(Secret::github_installation(t.token.expose().to_owned()))
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
            self.for_token(token)
                .create_org_repo(&gh(token), owner, &spec)
                .await
        } else {
            self.for_token(token)
                .create_user_repo(&gh(token), &spec)
                .await
        };
        Ok(repo(created.map_err(map_creation)?))
    }

    async fn branch_guard(
        &self,
        token: &Secret,
        owner: &str,
        name: &str,
        branch: &str,
    ) -> AppResult<BranchGuard> {
        let guard = self
            .for_token(token)
            .branch_guard(&gh(token), owner, name, branch)
            .await
            .map_err(map_err)?;
        Ok(BranchGuard {
            updates_restricted: guard.updates_restricted,
            deletion_restricted: guard.deletion_restricted,
        })
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
            restrict_updates: request.restrict_updates,
        };
        self.for_token(token)
            .create_branch_ruleset(&gh(token), owner, name, &spec)
            .await
            .map(|_| ())
            .map_err(map_err)
    }

    async fn grant_admin(
        &self,
        token: &Secret,
        owner: &str,
        name: &str,
        login: &str,
    ) -> AppResult<RoleGrant> {
        match self
            .for_token(token)
            .add_collaborator(&gh(token), owner, name, login, "admin")
            .await
            .map_err(map_err)?
        {
            CollaboratorAdded::Added => Ok(RoleGrant::Added),
            CollaboratorAdded::Invited => Ok(RoleGrant::Invited),
        }
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

    async fn create_issue(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        issue: &NewIssue,
    ) -> AppResult<GhIssue> {
        let spec = yulius_github::NewIssue {
            title: issue.title.clone(),
            body: issue.body.clone(),
            labels: issue.labels.clone(),
        };
        let opened = self
            .for_token(token)
            .create_issue(&gh(token), owner, repo_name, &spec)
            .await
            .map_err(map_err)?;
        Ok(GhIssue {
            number: opened.number,
            html_url: opened.url,
        })
    }

    async fn issue_open(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        number: u64,
    ) -> AppResult<bool> {
        self.for_token(token)
            .issue_open(&gh(token), owner, repo_name, number)
            .await
            .map_err(map_err)
    }

    async fn set_issue_state(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        number: u64,
        open: bool,
        comment: &str,
    ) -> AppResult<bool> {
        self.for_token(token)
            .set_issue_state(&gh(token), owner, repo_name, number, open, comment)
            .await
            .map_err(map_err)
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
            self.for_token(token)
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
            self.for_token(token)
                .get_pull(&gh(token), owner, repo_name, number)
                .await
                .map_err(map_err)?,
        ))
    }

    async fn confirm_pull(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        number: u64,
    ) -> AppResult<GhPull> {
        Ok(pull(
            self.plain
                .confirm_pull(&gh(token), owner, repo_name, number)
                .await
                .map_err(map_err)?,
        ))
    }

    async fn update_pull_title(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        number: u64,
        title: &str,
    ) -> AppResult<()> {
        self.for_token(token)
            .update_pull_title(&gh(token), owner, repo_name, number, title)
            .await
            .map(|_| ())
            .map_err(map_refusal)
    }

    async fn list_pulls(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        head: Option<&str>,
    ) -> AppResult<Vec<GhPull>> {
        Ok(self
            .for_token(token)
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
            .for_token(token)
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
            .for_token(token)
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

    async fn create_branch(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        branch: &str,
        sha: &str,
    ) -> AppResult<()> {
        self.for_token(token)
            .create_branch_ref(&gh(token), owner, repo_name, branch, sha)
            .await
            .map_err(map_err)
    }

    async fn fast_forward(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        branch: &str,
        sha: &str,
    ) -> AppResult<()> {
        self.for_token(token)
            .fast_forward_ref(&gh(token), owner, repo_name, branch, sha)
            .await
            .map_err(map_err)
    }

    async fn create_commit_status(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        sha: &str,
        status: &NewCommitStatus,
    ) -> AppResult<()> {
        let status = CommitStatus {
            state: status.state.clone(),
            context: status.context.clone(),
            description: status.description.clone(),
            target_url: status.target_url.clone(),
        };
        match self
            .for_token(token)
            .create_commit_status(&gh(token), owner, repo_name, sha, &status)
            .await
        {
            Ok(()) => Ok(()),
            Err(GitHubError::Forbidden { message, .. })
                if message
                    .to_ascii_lowercase()
                    .contains("not accessible by integration") =>
            {
                Err(AppError::Forbidden(STATUS_PERMISSION_MISSING.to_owned()))
            }
            Err(err) => Err(map_err(err)),
        }
    }

    async fn delete_branch(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        branch: &str,
    ) -> AppResult<bool> {
        self.for_token(token)
            .delete_branch(&gh(token), owner, repo_name, branch)
            .await
            .map_err(map_err)
    }

    async fn branch_pulls(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        branch: &str,
    ) -> AppResult<Vec<GhPull>> {
        let mut pulls: Vec<GhPull> = self
            .for_token(token)
            .list_pulls(&gh(token), owner, repo_name, PullFilter::All, Some(branch))
            .await
            .map_err(map_err)?
            .into_iter()
            .map(pull)
            .collect();
        pulls.sort_by_key(|p| std::cmp::Reverse(p.number));
        Ok(pulls)
    }

    async fn branch_sha(
        &self,
        token: &Secret,
        owner: &str,
        repo_name: &str,
        branch: &str,
    ) -> AppResult<Option<String>> {
        self.for_token(token)
            .branch_head(&gh(token), owner, repo_name, branch)
            .await
            .map_err(map_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_creation_carries_the_permissions_github_accepts() {
        let refused = |accepted: Option<&str>| GitHubError::Forbidden {
            message: "Resource not accessible by integration".into(),
            accepted_permissions: accepted.map(str::to_owned),
        };
        assert!(matches!(
            map_creation(refused(Some("administration=write"))),
            AppError::Forbidden(accepted) if accepted == "administration=write"
        ));
        assert!(matches!(
            map_creation(refused(None)),
            AppError::Forbidden(accepted) if accepted.is_empty()
        ));
        // Another refusal (an organization's policy) keeps GitHub's words.
        let policy = map_creation(GitHubError::Forbidden {
            message: "Members cannot create private repositories".into(),
            accepted_permissions: None,
        });
        assert!(
            matches!(&policy, AppError::Upstream(text) if text.contains("Members cannot create")),
            "{policy:?}"
        );
        assert!(matches!(
            map_creation(GitHubError::Invalid {
                message: "name already exists on this account".into()
            }),
            AppError::Upstream(_)
        ));
    }

    /// SPEC-069 BPL-008: only failures that may pass by themselves stay `Upstream`, so a title
    /// GitHub refused stops the merge and a rate limit or an outage only delays it.
    #[test]
    fn a_refused_write_is_told_from_one_that_may_pass() {
        let message = || "Validation Failed".to_owned();
        for refused in [
            GitHubError::Invalid { message: message() },
            GitHubError::Forbidden {
                message: message(),
                accepted_permissions: None,
            },
            GitHubError::Upstream(400),
            GitHubError::NotFound,
            GitHubError::Unauthorized,
        ] {
            let mapped = map_refusal(refused);
            assert!(!matches!(mapped, AppError::Upstream(_)), "{mapped:?}");
        }
        for passing in [
            GitHubError::RateLimited { retry_after: None },
            GitHubError::Upstream(502),
            GitHubError::Upstream(429),
            GitHubError::Upstream(408),
            GitHubError::Transport("timed out".into()),
            GitHubError::Decode("truncated".into()),
        ] {
            let mapped = map_refusal(passing);
            assert!(matches!(mapped, AppError::Upstream(_)), "{mapped:?}");
        }
    }
}
