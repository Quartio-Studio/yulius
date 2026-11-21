//! Task-scoped post-merge evidence (SPEC-562).
pub use yulius_contracts::api::git::{EvidenceRun, MergeEvidence};

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::github::GhPull;
use yulius_domain::permission::Action;

/// The observation backing a possible landing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandingSource {
    /// A successful local merge or a confirmed GitHub merge response.
    MergeCommand,
    /// A closed GitHub PR observed later. Its base must match the original task PR intent.
    ClosedPull,
    /// The submitted branch and head and the SHA returned by an unconfirmed merge command.
    AwaitingConfirmation,
}

/// The accepted PR head and the commit actually landed on its base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landing {
    /// Base branch at the merge.
    pub branch: String,
    /// Accepted head.
    pub head: String,
    /// Result of the merge.
    pub sha: String,
    /// How this identity was established.
    pub source: LandingSource,
}

impl Landing {
    /// Only full commit IDs can bind historical proof. An ambiguous playground recovery
    /// ("already there") or a missing merge result supplies no identity.
    pub(crate) fn new(branch: &str, head: &str, sha: &str) -> Option<Self> {
        (super::is_hex_sha(head) && super::is_hex_sha(sha)).then(|| Self {
            branch: branch.to_owned(),
            head: head.to_owned(),
            sha: sha.to_owned(),
            source: LandingSource::MergeCommand,
        })
    }

    /// The landing identity reported by a terminal pull, when it supplies both full commit IDs.
    #[must_use]
    pub fn closed_pull(pull: &GhPull) -> Option<Self> {
        if !pull.merged || pull.from_fork {
            return None;
        }
        let mut landed = Self::new(
            &pull.base,
            &pull.head_sha,
            pull.merge_commit_sha.as_deref()?,
        )?;
        landed.source = LandingSource::ClosedPull;
        Some(landed)
    }

    /// The merge API guards a head and returns a SHA. A fresh view of the closed PR
    /// confirms the base on which that result landed.
    pub(crate) fn confirmed_merge(
        pull: &GhPull,
        integration: &str,
        number: u64,
        head: &str,
        sha: &str,
    ) -> Option<Self> {
        let mut landed = Self::closed_pull(pull)?;
        landed.source = LandingSource::MergeCommand;
        (pull.number == number
            && landed.branch == integration
            && landed.head == head
            && landed.sha == sha)
            .then_some(landed)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_merge_response_binds_only_the_confirmed_pull_base_head_and_result() {
        let confirmed = GhPull {
            number: 7,
            html_url: String::new(),
            title: "Setup guide".into(),
            state: "closed".into(),
            merged: true,
            base: "dev".into(),
            base_sha: String::new(),
            head: "docs/setup".into(),
            head_sha: "a".repeat(40),
            mergeable: None,
            behind: false,
            from_fork: false,
            merge_commit_sha: Some("b".repeat(40)),
        };
        let seen = confirmed.head_sha.clone();
        let landed = "b".repeat(40);
        let evidence = |pr: &GhPull| Landing::confirmed_merge(pr, "dev", 7, &seen, &landed);
        assert_eq!(
            evidence(&confirmed),
            Some(Landing {
                branch: "dev".into(),
                head: seen.clone(),
                source: LandingSource::MergeCommand,
                sha: landed.clone()
            })
        );
        for pr in [
            GhPull {
                number: 8,
                ..confirmed.clone()
            },
            GhPull {
                head_sha: "c".repeat(40),
                ..confirmed.clone()
            },
            GhPull {
                merge_commit_sha: Some("d".repeat(40)),
                ..confirmed.clone()
            },
            GhPull {
                merge_commit_sha: None,
                ..confirmed.clone()
            },
            GhPull {
                from_fork: true,
                ..confirmed.clone()
            },
            GhPull {
                merged: false,
                ..confirmed.clone()
            },
            GhPull {
                base: "other".into(),
                ..confirmed
            },
        ] {
            assert!(evidence(&pr).is_none());
        }
    }
}
