//! Task-scoped post-merge evidence (SPEC-562).
pub use yulius_contracts::api::git::{EvidenceRun, MergeEvidence};

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::github::GhPull;
use yulius_domain::permission::Action;

/// The accepted PR head and the commit actually landed on its base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landing {
    /// Base branch at the merge.
    pub branch: String,
    /// Accepted head.
    pub head: String,
    /// Result of the merge.
    pub sha: String,
}

impl Landing {
    /// Only full commit IDs can bind historical proof. An ambiguous playground recovery
    /// ("already there") or a missing merge result supplies no identity.
    pub(crate) fn new(branch: &str, head: &str, sha: &str) -> Option<Self> {
        (super::is_hex_sha(head) && super::is_hex_sha(sha)).then(|| Self {
            branch: branch.to_owned(),
            head: head.to_owned(),
            sha: sha.to_owned(),
        })
    }

    pub(crate) fn of_pull(pull: &GhPull, integration: &str) -> Option<Self> {
        if !pull.merged || pull.from_fork || pull.base != integration {
            return None;
        }
        Self::new(
            &pull.base,
            &pull.head_sha,
            pull.merge_commit_sha.as_deref()?,
        )
    }
}

impl App {
    /// Read evidence without consulting a moving ref or requiring the worktree or GitHub.
    ///
    /// # Errors
    /// `NotFound` without task read access or the requested proof.
    pub async fn task_merge_evidence(
        &self,
        ctx: &AuthContext,
        task_id: &str,
        sha: &str,
        run_id: Option<&str>,
        before: Option<&str>,
    ) -> AppResult<MergeEvidence> {
        let task = self.load_task(ctx, task_id, Action::View).await?;
        self.deps
            .store
            .merge_evidence(&task.id, sha, run_id, before)
            .await?
            .ok_or(AppError::NotFound)
    }
}
