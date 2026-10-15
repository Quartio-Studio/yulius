//! A playground's delivery (SPEC-099 PLY-007, ADR-0081): no pull request and no GitHub.
//! `delivery_merge` and the task's merge action squash the task branch onto the playground's
//! integration branch in its own `origin`, after the same task gates as a pull request (a valid
//! spec where one is required, a plan with every step resolved, a clean worktree, commits to
//! deliver) and, when the playground has CI, once Yulius CI passed on the head merged into the
//! integration branch (SPEC-163, ADR-0134), with the title a pull request would get (SPEC-069
//! BPL-008).

use std::path::Path;

use yulius_contracts::api::git::Delivery;
use yulius_domain::ci::RunState;
use yulius_domain::delivery::DeliveryStage;

use super::{
    DeliveryRecord, default_pr_title, delivery_profile, max_title_chars, past_merge, sent_title,
};
use crate::ci::playground::TaskChecks;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{TaskRecord, UserRecord};
use crate::ports::git::{SquashLanding, SquashRequest};

/// Most branch commits read for the squash's title.
const MAX_SUBJECTS: u32 = 50;

/// How a playground task's merge went (SPEC-163 PCI-003).
#[derive(Debug)]
pub(crate) enum PlaygroundMerge {
    /// Squashed onto the integration branch.
    Merged(Box<Delivery>),
    /// Yulius CI is testing the head merged into the integration branch: the run's state, and a
    /// sentence saying so.
    Testing(RunState, String),
}

/// The refusal of a branch that conflicts with the integration branch in `files`.
fn conflict_refusal(branch: &str, integration: &str, files: &[String]) -> AppError {
    AppError::Conflict(format!(
        "`{branch}` conflicts with `{integration}` in {}: merge it into your branch \
         (`git merge origin/{integration}`; Yulius has just brought it up to date), \
         resolve the conflicts, commit, rerun the gates and merge again",
        if files.is_empty() {
            "some files".to_owned()
        } else {
            files.join(", ")
        }
    ))
}

impl App {
    /// [`App::playground_merge`] for a person, who never arms a merge: while Yulius CI tests it,
    /// it is a `Conflict` saying so (SPEC-163 PCI-004).
    ///
    /// # Errors
    /// As [`App::playground_merge`].
    pub(crate) async fn playground_merge_now(
        &self,
        user: &UserRecord,
        task: &TaskRecord,
    ) -> AppResult<Delivery> {
        let sandbox = self.dlv_sandbox_of(task).await?;
        let pass = self.armed_pass(&sandbox.id);
        let _pass = pass.lock().await;
        match self.playground_merge(user, task, None).await? {
            PlaygroundMerge::Merged(view) => Ok(*view),
            PlaygroundMerge::Testing(_, note) => Err(AppError::Conflict(format!(
                "not merged yet: {note}; merge again once it passed"
            ))),
        }
    }

    /// Squashes `task`'s branch onto its playground's integration branch as `user` (PLY-007), once
    /// Yulius CI passed on its head merged into the integration branch (SPEC-163): while that run
    /// goes, nothing lands and it says so. Callers hold the sandbox's armed-merge pass
    /// ([`App::armed_pass`]).
    ///
    /// # Errors
    /// `Conflict` with what to do when the task is not ready, already merged, has nothing to
    /// deliver, conflicts with the integration branch or failed its checks; Git failures.
    pub(crate) async fn playground_merge(
        &self,
        user: &UserRecord,
        task: &TaskRecord,
        reviewed: Option<&str>,
    ) -> AppResult<PlaygroundMerge> {
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
        // Merge on green (SPEC-163 PCI-003): the head merged into the integration branch as it
        // is now, tested by Yulius CI when the playground has CI.
        match self.playground_task_checks(&sandbox, task, &head).await? {
            TaskChecks::Clear => {}
            TaskChecks::Testing(state) => {
                return Ok(PlaygroundMerge::Testing(
                    state,
                    format!(
                        "Yulius CI is testing `{branch}` at {} merged into `{integration}` (Task \
                         #{}: {})",
                        short(&head),
                        task.number,
                        state.as_str()
                    ),
                ));
            }
            TaskChecks::Failed(run) => {
                return Err(AppError::Conflict(format!(
                    "not merged: merge on green only: Yulius CI {} on `{branch}` at {} merged into \
                     `{integration}` (run {}); read why with `ci_status`, fix it test-first and \
                     commit",
                    run.state.as_str(),
                    short(&head),
                    run.id
                )));
            }
            TaskChecks::Conflict(files) => {
                return Err(conflict_refusal(branch, integration, &files));
            }
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
            // The branch changes something, and the integration branch already has it all (a
            // squash that landed before its stage was recorded): it is merged. A branch whose
            // commits cancel out delivers nothing.
            SquashLanding::Unchanged => {
                let since = git
                    .merge_base(&repo, &base, &head)
                    .await?
                    .ok_or_else(nothing)?;
                if git.diff_stat(&repo, &since, &head).await?.files == 0 {
                    return Err(nothing());
                }
                format!("{base} (already there)")
            }
            SquashLanding::Conflict(files) => {
                return Err(conflict_refusal(branch, integration, &files));
            }
        };
        let next = DeliveryRecord {
            task_id: task.id.clone(),
            stage: DeliveryStage::MergedIntegration,
            pr_number: None,
            updated_at: self.now(),
        };
        if !self
            .dlv_transition(&task.sandbox_id, &observed.record.stage, &next)
            .await?
        {
            // Changed meanwhile (another merge of this task): the view below shows what it is.
            tracing::warn!(task = %task.id, "a playground merge found the delivery stage changed");
        }
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
        Ok(PlaygroundMerge::Merged(Box::new(
            self.delivery_view(&task, Some(user)).await?,
        )))
    }
}

fn short(sha: &str) -> &str {
    sha.get(..9).unwrap_or(sha)
}
