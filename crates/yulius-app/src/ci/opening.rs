//! Queue a signed opening without asking GitHub to list or read pull requests (SPEC-606).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::OwnedSemaphorePermit;
use yulius_domain::ci::{self, CiRef, Target};
use yulius_domain::time::parse_rfc3339;

use super::{MAX_PULLS_TESTED, PLAN_RUNS, ordinary_run_facts};
use crate::context::App;
use crate::delivery::delivery_profile;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::github::GhPull;

pub(super) const WEBHOOK_ACTOR: &str = "GitHub webhook";

/// A GitHub-authenticated opening whose head and base both belong to the stated repository.
pub struct PullOpening {
    /// GitHub installation that received the event.
    pub installation: u64,
    /// GitHub owner/name.
    pub repository: String,
    /// Repository's stable GitHub ID.
    pub repo_id: u64,
    /// Number of the pull request.
    pub number: u64,
    /// Branch into which the request would merge.
    pub base: String,
    /// Branch carrying the proposed change.
    pub branch: String,
    /// Exact commit when the request opened.
    pub sha: String,
    /// GitHub's creation time, covered by its signature.
    pub created_at: String,
}

/// Admitted open PRs in stable order. Openings observed during a REST request take
/// precedence over its older snapshot. Both collections are bounded by MAX_PULLS_TESTED.
#[derive(Default)]
pub(super) struct PullWindow {
    pub(super) heads: Vec<(u64, String)>,
    pub(super) since_listing: HashSet<u64>,
    restored: bool,
}

impl PullWindow {
    fn has_room(&self, number: u64) -> bool {
        self.heads.len() < MAX_PULLS_TESTED || self.heads.iter().any(|(n, _)| *n == number)
    }

    fn admit(&mut self, number: u64, sha: &str) {
        if let Some((_, head)) = self.heads.iter_mut().find(|(n, _)| *n == number) {
            sha.clone_into(head);
        } else if self.heads.len() < MAX_PULLS_TESTED {
            self.heads.push((number, sha.to_owned()));
        }
    }

    pub(super) fn reconcile(&mut self, pulls: &[GhPull]) {
        self.heads.retain(|(n, _)| {
            self.since_listing.contains(n) || pulls.iter().any(|p| p.number == *n)
        });
        for pull in pulls {
            if !self.since_listing.contains(&pull.number) {
                self.admit(pull.number, &pull.head_sha);
            }
        }
    }

    pub(super) async fn restore(&mut self, app: &App, sandbox: &SandboxRecord) -> AppResult<()> {
        if !self.restored {
            for run in app.deps.store.active_ci_runs().await? {
                if run.sandbox_id == sandbox.id
                    && let CiRef::PullRequest(number) = run.reference
                {
                    self.admit(number, &run.sha);
                }
            }
            self.restored = true;
        }
        Ok(())
    }
}

impl App {
    /// Reserves one delivery from the first HTTP body read through its scheduling attempt.
    ///
    /// # Errors
    /// All delivery slots are occupied.
    pub fn ci_webhook_permit(&self) -> AppResult<OwnedSemaphorePermit> {
        self.webhook_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| AppError::Capacity("CI is handling other GitHub deliveries".into()))
    }

    /// Admit a fresh opening for this installation. The periodic sync recovers a missed event,
    /// busy server or failed fetch. Git and CI continue after GitHub's HTTP request ends.
    ///
    /// # Errors
    /// Store failures or the bounded webhook worker pool being full.
    pub async fn ci_pull_opened(
        self: &Arc<Self>,
        opening: PullOpening,
        permit: OwnedSemaphorePermit,
    ) -> AppResult<()> {
        let opened_at = parse_rfc3339(&opening.created_at)
            .ok_or_else(|| AppError::Invalid("invalid pull request opening time".into()))?;
        if !self.config.ci.enabled
            || !(-60_000..=60_000).contains(&self.now().millis_since(opened_at))
            || !self.deps.git.valid_branch(&opening.branch)
            || !matches!(opening.sha.len(), 40 | 64)
            || !opening.sha.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Ok(());
        }
        let Some((owner, name)) = opening.repository.split_once('/') else {
            return Ok(());
        };
        let store = &self.deps.store;
        if store.instance().await?.and_then(|i| i.installation_id) != Some(opening.installation) {
            return Ok(());
        }
        let Some(sandbox) = store.sandbox_by_repo(owner, name).await? else {
            return Ok(());
        };
        if sandbox.github_repo_id != Some(opening.repo_id)
            || sandbox.is_playground()
            || sandbox.removed_at.is_some()
            || opening.base != delivery_profile(&sandbox).integration_branch
        {
            return Ok(());
        }
        let app = Arc::clone(self);
        let worker = tokio::spawn(async move {
            let _permit = permit;
            let result = app.ci_queue_opening(&sandbox, &opening).await;
            if let Err(err) = &result {
                tracing::warn!(sandbox = %sandbox.slug, pr = opening.number, error = %err, "CI webhook scheduling failed");
            }
            result
        });
        // Return an immediate failure when known. A slow fetch continues under its permit
        // after acknowledgement, keeping the response inside GitHub's ten-second deadline.
        match tokio::time::timeout(Duration::from_secs(3), worker).await {
            Ok(result) => result.map_err(AppError::internal)?,
            Err(_) => Ok(()),
        }
    }

    async fn ci_queue_opening(
        &self,
        sandbox: &SandboxRecord,
        opening: &PullOpening,
    ) -> AppResult<()> {
        let lock = self.ci_sync_lock(&sandbox.id);
        let Ok(mut window) =
            tokio::time::timeout(Duration::from_secs(60), lock.schedule.lock()).await
        else {
            return Err(AppError::Capacity("the sandbox CI sync is busy".into()));
        };
        // Removal or a profile edit can win while this attempt waits for another writer.
        let Some(sandbox) = self.deps.store.sandbox(&sandbox.id).await? else {
            return Ok(());
        };
        if sandbox.removed_at.is_some()
            || sandbox.github_repo_id != Some(opening.repo_id)
            || delivery_profile(&sandbox).integration_branch != opening.base
        {
            return Ok(());
        }
        window.restore(self, &sandbox).await?;
        if !window.has_room(opening.number) {
            return Ok(());
        }
        self.dlv_refresh_clone(&sandbox).await?;
        let repo = self.sandbox_repo(&sandbox.id);
        let branch = format!("refs/remotes/origin/{}", opening.branch);
        if self
            .deps
            .git
            .resolve_commit(&repo, &branch)
            .await?
            .as_deref()
            != Some(&opening.sha)
        {
            return Ok(());
        }
        self.ci_forget_gate(&sandbox.id);
        let view = self.ci_gate(&sandbox).await;
        let target = Target {
            reference: CiRef::PullRequest(opening.number),
            sha: opening.sha.clone(),
        };
        let runs = self
            .deps
            .store
            .ci_planning_runs(&sandbox.id, PLAN_RUNS)
            .await?;
        let facts = ordinary_run_facts(&runs, view.problem.is_none());
        window.admit(opening.number, &opening.sha);
        window.since_listing.insert(opening.number);
        // Only the full inventory can supersede runs. This event describes one target.
        if !ci::plan(std::slice::from_ref(&target), &facts)
            .queue
            .is_empty()
            && let Some(gate) = self.ci_gate_for_head(&sandbox, &view, &target.sha).await
        {
            let run = self
                .ci_record_placed(
                    &sandbox,
                    target.reference,
                    &target.sha,
                    &gate,
                    WEBHOOK_ACTOR,
                    self.now(),
                )
                .await?;
            drop(window);
            self.ci_publish(&run.id).await;
            self.ci_wake();
        }
        Ok(())
    }
}
