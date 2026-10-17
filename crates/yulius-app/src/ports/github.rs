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
    /// Only repository admins may update the branch: the production branch (ADR-0114).
    pub restrict_updates: bool,
}

/// Whether a token could write a branch, as GitHub's rules say (ADR-0114).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BranchGuard {
    /// An active `update` rule applies that the token can never bypass.
    pub updates_restricted: bool,
    /// An active `deletion` rule applies that the token can never bypass.
    pub deletion_restricted: bool,
}

impl BranchGuard {
    /// The token can neither move nor delete the branch.
    #[must_use]
    pub fn is_guarded(self) -> bool {
        self.updates_restricted && self.deletion_restricted
    }
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
    /// Cannot be known: no source could be read, or nothing was reported while a source could
    /// not be read. Never counts as passed.
    Unknown,
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
            Self::Unknown => "unknown",
        }
    }
}

/// Where GitHub reports CI results, each behind its own App permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ChecksSource {
    /// Check runs (GitHub Actions and every Checks API app): "Checks: read".
    CheckRuns,
    /// Commit statuses (CI that reports statuses): "Commit statuses: read".
    CommitStatuses,
    /// GitHub Actions workflow runs, read only when check runs cannot be: "Actions: read".
    WorkflowRuns,
}

impl ChecksSource {
    /// The App permission that reads this source, as GitHub's settings name it.
    #[must_use]
    pub fn permission(self) -> &'static str {
        match self {
            Self::CheckRuns => "Checks",
            Self::CommitStatuses => "Commit statuses",
            Self::WorkflowRuns => "Actions",
        }
    }

    /// What the source holds, in words.
    #[must_use]
    pub fn noun(self) -> &'static str {
        match self {
            Self::CheckRuns => "check runs",
            Self::CommitStatuses => "commit statuses",
            Self::WorkflowRuns => "Actions runs",
        }
    }
}

/// The checks of one commit and the sources the App may not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksReport {
    /// Aggregate over the readable sources.
    pub state: ChecksState,
    /// Sources the App lacks the permission for (sorted, without repeats).
    pub unreadable: Vec<ChecksSource>,
}

impl From<ChecksState> for ChecksReport {
    /// A report read from every source.
    fn from(state: ChecksState) -> Self {
        Self {
            state,
            unreadable: Vec::new(),
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
    /// GitHub's mergeability: `Some(false)` when it conflicts with its base; `None` while GitHub
    /// is still computing it or on list responses.
    pub mergeable: Option<bool>,
    /// Behind its base where the repository requires an up-to-date branch: GitHub refuses the
    /// merge ("required status checks are expected") until the base is merged in.
    pub behind: bool,
    /// The head is in another repository (a fork) or its repository is gone (SPEC-069).
    pub from_fork: bool,
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

/// A commit status to publish (SPEC-002 ADR-21).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCommitStatus {
    /// `pending`, `success`, `failure` or `error`.
    pub state: String,
    /// Context (`yulius/ci`).
    pub context: String,
    /// Description (at most 140 characters).
    pub description: String,
    /// Link to the run in Yulius.
    pub target_url: Option<String>,
}

/// Why GitHub refused to publish a commit status.
pub const STATUS_PERMISSION_MISSING: &str =
    "the GitHub App needs the \u{201c}Commit statuses: Read and write\u{201d} permission";
/// An issue to open (DLV-008).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewIssue {
    /// Title.
    pub title: String,
    /// Body (Markdown).
    pub body: String,
    /// Label names; labels the repository lacks are created.
    pub labels: Vec<String>,
}

/// An opened issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhIssue {
    /// Number.
    pub number: u64,
    /// Web URL.
    pub html_url: String,
}

/// GitHub operations used by the application.
#[async_trait]
pub trait GitHubPort: Send + Sync {
    /// OAuth authorization URL for user sign-in; `select_account` makes GitHub show its account
    /// picker (to continue with another GitHub account than the one signed in to github.com).
    fn authorize_url(
        &self,
        app: &GitHubAppConfig,
        state: &str,
        redirect_uri: &str,
        select_account: bool,
    ) -> String;
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
    /// The token of an agent's Git and `gh` (ADR-0114): an installation token restricted to the
    /// repository `repo` and to the agent permissions (no administration, no workflows), which
    /// GitHub refuses on a production branch protected by the production ruleset.
    ///
    /// # Errors
    /// GitHub failures.
    async fn agent_token(
        &self,
        app: &GitHubAppConfig,
        installation_id: u64,
        repo: &str,
    ) -> AppResult<Secret>;
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
    /// Whether `token` could write `branch` of `owner/name`: GitHub's branch rules and whether
    /// the token may bypass them (ADR-0114).
    ///
    /// # Errors
    /// GitHub failures: an unreadable rule is never "guarded".
    async fn branch_guard(
        &self,
        token: &Secret,
        owner: &str,
        name: &str,
        branch: &str,
    ) -> AppResult<BranchGuard>;
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
    /// Opens an issue with its labels, creating labels the repository lacks.
    async fn create_issue(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        issue: &NewIssue,
    ) -> AppResult<GhIssue>;
    /// Reopens (`open`) or closes issue `number`, first commenting `comment`, unless it already
    /// is in that state; whether it changed (SPEC-217 VSI-004).
    async fn set_issue_state(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        number: u64,
        open: bool,
        comment: &str,
    ) -> AppResult<bool>;
    /// Gets a pull request.
    async fn get_pull(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        number: u64,
    ) -> AppResult<GhPull>;
    /// Sets the title of a pull request (SPEC-069 BPL-008).
    ///
    /// # Errors
    /// `Upstream` only for failures that may pass by themselves (a rate limit, a 5xx, a timeout,
    /// the network, an unreadable answer); GitHub's refusal (a 4xx) is `Invalid`, `Forbidden`,
    /// `NotFound`, `Unauthenticated` or `Conflict`.
    async fn update_pull_title(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        number: u64,
        title: &str,
    ) -> AppResult<()>;
    /// Open pull requests (optionally for a head branch).
    async fn list_pulls(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        head: Option<&str>,
    ) -> AppResult<Vec<GhPull>>;
    /// Aggregated checks of a commit, read from every source the App may read (a missing
    /// permission is reported in [`ChecksReport::unreadable`], not as an error).
    async fn checks(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> AppResult<ChecksReport>;
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
    /// Publishes a commit status (App permission "Commit statuses: write"). A missing
    /// permission is `Forbidden` with [`STATUS_PERMISSION_MISSING`].
    ///
    /// # Errors
    /// GitHub failures.
    async fn create_commit_status(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        sha: &str,
        status: &NewCommitStatus,
    ) -> AppResult<()> {
        let _ = (token, owner, repo, sha, status);
        Err(crate::error::AppError::Upstream(
            "publishing commit statuses is not available".into(),
        ))
    }
    /// Deletes a branch (App permission "Contents: write"); `Ok(false)` when it did not exist.
    ///
    /// # Errors
    /// GitHub failures.
    async fn delete_branch(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> AppResult<bool> {
        let _ = (token, owner, repo, branch);
        Err(crate::error::AppError::Upstream(
            "deleting branches is not available".into(),
        ))
    }
    /// Pull requests (open, closed and merged) whose head is `branch` of the repository owner,
    /// newest first.
    ///
    /// # Errors
    /// GitHub failures.
    async fn branch_pulls(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> AppResult<Vec<GhPull>> {
        let _ = (token, owner, repo, branch);
        Err(crate::error::AppError::Upstream(
            "listing pull requests by branch is not available".into(),
        ))
    }
    /// Head SHA of a branch.
    async fn branch_sha(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        branch: &str,
    ) -> AppResult<Option<String>>;
}
