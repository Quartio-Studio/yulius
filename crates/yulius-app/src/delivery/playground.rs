//! A playground's delivery (SPEC-099 PLY-007, ADR-0081): no pull request, no Yulius CI and no
//! GitHub. `delivery_merge` and the task's merge action squash the task branch onto the
//! playground's integration branch in its own `origin`, after the same task gates as a pull
//! request (a valid spec where one is required, a plan with every step resolved, a clean worktree,
//! commits to deliver), with the title a pull request would get (SPEC-069 BPL-008).

use std::path::Path;

use yulius_contracts::api::git::Delivery;
use yulius_domain::delivery::DeliveryStage;

use super::{
    DeliveryRecord, default_pr_title, delivery_profile, max_title_chars, past_merge, sent_title,
};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{TaskRecord, UserRecord};
use crate::ports::git::{SquashLanding, SquashRequest};

/// Most branch commits read for the squash's title.
const MAX_SUBJECTS: u32 = 50;

impl App {
    /// Squashes `task`'s branch onto its playground's integration branch as `user` (PLY-007).
    ///
    /// # Errors
    /// `Conflict` with what to do when the task is not ready, already merged, has nothing to
    /// deliver or conflicts with the integration branch; Git failures.
    pub(crate) async fn playground_merge(
        &self,
        user: &UserRecord,
        task: &TaskRecord,
        reviewed: Option<&str>,
    ) -> AppResult<Delivery> {
        let sandbox = self.dlv_sandbox_of(task).await?;
        let profile = delivery_profile(&sandbox);
        let integration = profile.integration_branch.as_str();
        let (Some(branch), Some(worktree)) =
            (task.branch.as_deref(), task.worktree_path.as_deref())
        else {
            return Err(AppError::Conflict(
                "this task has no branch to deliver yet".into(),
            ));
        };
        let observed = self.dlv_observe(&sandbox, task, &profile, None).await?;
        let refusal = if past_merge(&observed.record.stage) {
            Some(format!("this task is already merged into `{integration}`"))
        } else if !observed.spec.valid {
            Some(format!(
                "the spec is not valid ({}); fix it and commit",
                observed.spec.problems.join(" ")
            ))
        } else if observed.steps.is_empty() && observed.spec.required {
            Some("the task has no plan: record its steps with `steps_set`".to_owned())
        } else if observed.steps_open > 0 {
            Some("a step is still open: finish it or resolve it with `step_update`".to_owned())
        } else if observed.dirty {
            Some("the worktree has uncommitted changes: commit them".to_owned())
        } else {
            None
        };
        if let Some(reason) = refusal {
            return Err(AppError::Conflict(format!("not merged: {reason}")));
        }
        let head = observed
            .worktree_head
            .clone()
            .ok_or_else(|| AppError::Conflict("the task branch has no commit".into()))?;
        // The agent's merge lands the head the review approved, nothing committed since.
        if let Some(reviewed) = reviewed
            && reviewed != head
        {
            return Err(AppError::Conflict(format!(
                "not merged: the branch moved from the reviewed head {reviewed} to {head}; review \
                 the new head and merge again"
            )));
        }
        // The integration branch as it is now in the playground's `origin`.
        self.dlv_refresh_clone(&sandbox).await?;
        let base = format!("origin/{integration}");
        let git = &self.deps.git;
        let nothing = || {
            AppError::Conflict(format!(
                "nothing to deliver: `{branch}` has no change that `{integration}` lacks"
            ))
        };
        let (ahead, _) = git.ahead_behind(Path::new(worktree), &base, "HEAD").await?;
        if ahead == 0 {
            return Err(nothing());
        }
        let subjects: Vec<String> = git
            .review_commits(Path::new(worktree), &base, &head, MAX_SUBJECTS)
            .await
            .map(|commits| commits.into_iter().map(|c| c.subject).collect())
            .unwrap_or_default();
        let (policy, _) = self.dlv_branch_policy(&sandbox, &profile).await;
        let title = sent_title(
            &default_pr_title(&task.title, &subjects, branch, &policy),
            max_title_chars(&policy),
        );
        let message = format!(
            "{title}\n\nTask #{} of the playground {}, squashed from `{branch}` at {head}.",
            task.number, sandbox.slug
        );
        let (name, email) = crate::projects::identity(user);
        let repo = self.sandbox_repo(&sandbox.id);
        let origin = self.playground_origin(&sandbox.id);
        let origin = origin.to_string_lossy();
        let landed = git
            .land_squash(
                &repo,
                &SquashRequest {
                    base: &base,
                    head: &head,
                    message: &message,
                    name: &name,
                    email: &email,
                    remote: &origin,
                    branch: integration,
                },
            )
            .await
            .map_err(|err| {
                let reason = err.public_message();
                if ["rejected", "fetch first", "non-fast-forward"]
                    .iter()
                    .any(|needle| reason.contains(needle))
                {
                    AppError::Conflict(format!(
                        "not merged: `{integration}` moved while this merge ran; merge again"
                    ))
                } else {
                    err
                }
            })?;
        let commit = match landed {
            SquashLanding::Landed(commit) => commit,
            // The branch's changes are already in the integration branch (a squash that landed
            // before its stage was recorded): it is merged.
            SquashLanding::Unchanged => format!("{base} (already there)"),
            SquashLanding::Conflict(files) => {
                return Err(AppError::Conflict(format!(
                    "`{branch}` conflicts with `{integration}` in {}: merge it into your branch \
                     (`git merge origin/{integration}`; Yulius has just brought it up to date), \
                     resolve the conflicts, commit, rerun the gates and merge again",
                    if files.is_empty() {
                        "some files".to_owned()
                    } else {
                        files.join(", ")
                    }
                )));
            }
        };
        let next = DeliveryRecord {
            task_id: task.id.clone(),
            stage: DeliveryStage::MergedIntegration,
            pr_number: None,
            updated_at: self.now(),
        };
        self.deps
            .store
            .transition_delivery(&observed.record.stage, &next)
            .await?;
        self.dlv_audit(
            &Self::dlv_actor_login(user),
            "delivery.merge",
            &format!("{}#{}", sandbox.slug, task.number),
            &format!(
                "playground: `{branch}` squashed into {integration} at head {head} -> {commit}"
            ),
        )
        .await;
        if let Err(err) = self.dlv_refresh_clone(&sandbox).await {
            tracing::warn!(error = %err, "refreshing a playground clone after a merge failed");
        }
        self.publish_task(&task.id).await;
        let task = self
            .deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.delivery_view(&task, Some(user)).await
    }
}
