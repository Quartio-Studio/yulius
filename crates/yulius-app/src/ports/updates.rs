//! Instance update ports (SPEC-002 ADR-18): reading `main` of the one repository, and the root
//! update helper that builds and installs a commit.

use async_trait::async_trait;
use yulius_contracts::update::{HelperRequest, HelperStatus};
use yulius_domain::update::Relation;

use crate::model::Secret;

/// One commit of `main` newer than the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCommit {
    /// Full id.
    pub sha: String,
    /// The whole message (titles are derived by the domain).
    pub message: String,
    /// Author's GitHub login, when GitHub knows it.
    pub author: Option<String>,
    /// Commit time (RFC 3339).
    pub date: Option<String>,
}

/// `main` as seen from the build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseState {
    /// Head of `main`.
    pub head: String,
    /// How the build relates to it.
    pub relation: Relation,
    /// Commits from the build (exclusive) to the head, newest first; for an unknown build, the
    /// latest commits of `main`. Bounded by the adapter.
    pub commits: Vec<RemoteCommit>,
}

/// Why `main` could not be read (no provider data, no credentials).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// The credential was refused or cannot see the repository (401, 403, 404).
    NoAccess,
    /// GitHub's rate limit; retry later.
    RateLimited,
    /// Network failure, timeout or a server error.
    Unavailable,
    /// An answer outside the expected shape (the failed check).
    Invalid(&'static str),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAccess => f.write_str("GitHub refused access to the repository"),
            Self::RateLimited => f.write_str("GitHub's rate limit was reached"),
            Self::Unavailable => f.write_str("GitHub could not be reached"),
            Self::Invalid(what) => write!(f, "GitHub's answer was not understood ({what})"),
        }
    }
}

/// Reads `main` of Quartio-Studio/yulius.
#[async_trait]
pub trait ReleaseSource: Send + Sync {
    /// The head of `main` and how `build` relates to it, read with `token` (anonymous without).
    ///
    /// # Errors
    /// [`SourceError`].
    async fn compare(
        &self,
        token: Option<&Secret>,
        build: Option<&str>,
    ) -> Result<ReleaseState, SourceError>;
}

/// The root update helper, reached through files (`yulius_contracts::update`).
pub trait UpdateHelper: Send + Sync {
    /// Whether the helper is installed on this server (its state directory exists).
    fn installed(&self) -> bool;
    /// Writes the request (atomically, owner-only), replacing an earlier one.
    ///
    /// # Errors
    /// A message when the file cannot be written.
    fn request(&self, request: &HelperRequest) -> Result<(), String>;
    /// The helper's last status, if readable and well formed.
    fn status(&self) -> Option<HelperStatus>;
    /// Removes the request file (the run it belongs to ended).
    fn clear_request(&self);
}
