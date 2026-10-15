//! Adapter-level GitHub types (provider wire types stay private in `wire`).

use yulius_domain::permission::{AccessLevel, GitHubPermissions};
use yulius_domain::time::Timestamp;

use crate::secret::Secret;

/// An installation access token (about one hour).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationToken {
    /// The token.
    pub token: Secret,
    /// Expiry reported by GitHub.
    pub expires_at: Timestamp,
}

/// A user-to-server OAuth token set. Expiry fields are `None` when the App does not use
/// expiring user tokens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserToken {
    /// Access token (about eight hours when expiring).
    pub access: Secret,
    /// Refresh token (about six months), single use.
    pub refresh: Option<Secret>,
    /// Access token expiry.
    pub expires_at: Option<Timestamp>,
    /// Refresh token expiry.
    pub refresh_expires_at: Option<Timestamp>,
}

/// The signed-in GitHub user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubUser {
    /// Stable numeric id (logins can be renamed).
    pub id: u64,
    /// Login.
    pub login: String,
    /// Display name.
    pub name: Option<String>,
    /// Avatar URL.
    pub avatar_url: Option<String>,
}

/// A repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// Stable numeric id.
    pub id: u64,
    /// Owner login.
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// `owner/name`.
    pub full_name: String,
    /// Private repository.
    pub private: bool,
    /// Default branch (`main` when GitHub omits it).
    pub default_branch: String,
    /// Web URL.
    pub html_url: String,
    /// HTTPS clone URL.
    pub clone_url: String,
}

/// A user's access to one repository (SPEC-001 ACC-007).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoAccess {
    /// GitHub answered 404: the repository is invisible to the user (not an error).
    NoAccess,
    /// The repository is visible with these permission flags.
    Granted {
        /// Flags as GitHub reports them.
        permissions: GitHubPermissions,
        /// The repository.
        repo: Repo,
    },
}

impl RepoAccess {
    /// The Yulius access level these permissions map to.
    #[must_use]
    pub fn level(&self) -> AccessLevel {
        match self {
            Self::NoAccess => AccessLevel::None,
            Self::Granted { permissions, .. } => AccessLevel::from_github(*permissions),
        }
    }
}

/// Repository creation request. Use [`CreateRepo::private`] for the Yulius defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRepo {
    /// Repository name.
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Private repository (Yulius default: true).
    pub private: bool,
    /// Let GitHub create an initial commit (Yulius default: false; the scaffold is pushed).
    pub auto_init: bool,
}

impl CreateRepo {
    /// A private, empty repository named `name`.
    #[must_use]
    pub fn private(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
            private: true,
            auto_init: false,
        }
    }
}

/// Pull request merge method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MergeMethod {
    /// Merge commit.
    Merge,
    /// Squash merge.
    Squash,
    /// Rebase merge.
    Rebase,
}

impl MergeMethod {
    /// GitHub wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Squash => "squash",
            Self::Rebase => "rebase",
        }
    }
}

/// A branch ruleset to create (DSC-007).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulesetSpec {
    /// Ruleset name.
    pub name: String,
    /// Protected branch (short name).
    pub branch: String,
    /// Require a pull request for every change.
    pub require_pr: bool,
    /// Merge methods allowed through pull requests (empty: GitHub default, all).
    pub allowed_merge_methods: Vec<MergeMethod>,
    /// Block force pushes (`non_fast_forward` rule).
    pub block_force_push: bool,
    /// Block branch deletion.
    pub block_deletion: bool,
    /// Restrict updates (`update` rule) with repository admins as the only bypass actors: GitHub
    /// then refuses every other actor's push, merge or fast-forward, the App's tokens included
    /// (ADR-0114).
    pub restrict_updates: bool,
}

/// Pull request state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PullState {
    /// Open.
    Open,
    /// Closed without merging.
    Closed,
    /// Merged.
    Merged,
}

impl PullState {
    /// API contract name (`open|merged|closed`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Merged => "merged",
        }
    }
}

/// Filter for [`crate::GitHubClient::list_pulls`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PullFilter {
    /// Open pull requests.
    Open,
    /// Closed (including merged) pull requests.
    Closed,
    /// All pull requests.
    All,
}

impl PullFilter {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::All => "all",
        }
    }
}

/// A pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    /// Number.
    pub number: u64,
    /// Web URL.
    pub url: String,
    /// Title.
    pub title: String,
    /// State (merged derived from `merged_at`).
    pub state: PullState,
    /// Base branch.
    pub base: String,
    /// Base commit at the time GitHub computed the pull request.
    pub base_sha: String,
    /// Head branch.
    pub head: String,
    /// Head commit.
    pub head_sha: String,
    /// Draft pull request.
    pub draft: bool,
    /// GitHub's mergeability (`None` while it is still computing or on list responses).
    pub mergeable: Option<bool>,
    /// GitHub's `mergeable_state` is `behind`: the base moved on and the repository requires the
    /// branch to be up to date before it merges (single reads only).
    pub behind: bool,
    /// The head is in another repository than the base (a fork), or its repository is gone.
    pub cross_repository: bool,
    /// Merge commit after merging.
    pub merge_commit_sha: Option<String>,
}

/// Request to open a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPull {
    /// Head branch (in the same repository).
    pub head: String,
    /// Base branch.
    pub base: String,
    /// Title.
    pub title: String,
    /// Body (Markdown).
    pub body: String,
}

/// Request to open an issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewIssue {
    /// Title.
    pub title: String,
    /// Body (Markdown).
    pub body: String,
    /// Label names; missing ones are created.
    pub labels: Vec<String>,
}

/// An opened issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueRef {
    /// Number.
    pub number: u64,
    /// Web URL.
    pub url: String,
}

/// Aggregate of check runs and the combined commit status for one commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChecksSummary {
    /// No check runs and no statuses.
    None,
    /// Something is queued or running and nothing has failed.
    Pending,
    /// Everything completed successfully (success, neutral or skipped).
    Success,
    /// Any failure, error, cancellation, timeout, required action or stale run.
    Failure,
    /// The checks cannot be known: no source could be read, or the readable ones reported
    /// nothing while another could not be read. Never counts as passed.
    Unknown,
}

impl ChecksSummary {
    /// API contract name (`none|pending|success|failure|unknown`).
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

/// Where GitHub reports a commit's CI results, each behind its own App permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CheckSource {
    /// Check runs (`GET …/commits/{sha}/check-runs`): GitHub Actions and every Checks API app.
    /// App permission "Checks: read".
    CheckRuns,
    /// The combined commit status (`GET …/commits/{sha}/status`): CI that reports statuses.
    /// App permission "Commit statuses: read".
    CommitStatuses,
    /// GitHub Actions workflow runs of the commit (`GET …/actions/runs?head_sha=`), read only
    /// when check runs cannot be. App permission "Actions: read".
    WorkflowRuns,
}

impl CheckSource {
    /// The App permission that reads this source, as GitHub's settings name it.
    #[must_use]
    pub fn permission(self) -> &'static str {
        match self {
            Self::CheckRuns => "Checks",
            Self::CommitStatuses => "Commit statuses",
            Self::WorkflowRuns => "Actions",
        }
    }
}

/// The CI state of one commit with the sources that could not be read (HTTP 403 "Resource not
/// accessible by integration": the App lacks the source's permission).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksReport {
    /// Aggregate over the sources that could be read ([`ChecksSummary::Unknown`] when that
    /// cannot tell).
    pub summary: ChecksSummary,
    /// Sources the App may not read, in [`CheckSource`] order.
    pub unreadable: Vec<CheckSource>,
}

/// Result of a successful merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    /// The resulting commit on the base branch.
    pub sha: String,
}

/// Relationship of `head` to `base` in [`crate::GitHubClient::compare`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompareStatus {
    /// Same commit.
    Identical,
    /// `head` is strictly ahead of `base`.
    Ahead,
    /// `head` is strictly behind `base`.
    Behind,
    /// Both have unique commits.
    Diverged,
}

/// Commit comparison of `base...head`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comparison {
    /// Commits in `head` not in `base`.
    pub ahead_by: u64,
    /// Commits in `base` not in `head`.
    pub behind_by: u64,
    /// Overall relationship.
    pub status: CompareStatus,
}
