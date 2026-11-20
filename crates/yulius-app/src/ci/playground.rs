//! Yulius CI of a playground's task (SPEC-163 PCI-002): a playground has no pull request, so its
//! task's head merged into the integration branch is built in Yulius's clone, as a merge train
//! builds its commit (SPEC-084), and tested as `task/<n>` before `delivery_merge` squashes it.
//! The build is a merge commit whose parents name what it tests: the integration head first, then
//! the head itself or Yulius's merge of the integration head into it (whose parents are the head
//! and the integration head). A head's run is the newest whose commit has exactly those parents, never one of a later
//! commit that contains it (review round 1). Once it passed, the squash lands that commit's tree,
//! so the integration head after it reuses the result.

use std::path::Path;

use yulius_domain::ci::{CiRef, RunState};

use super::{CiRunRecord, GateView, PLAN_RUNS, short};
use crate::context::App;
use crate::delivery::delivery_profile;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, TaskRecord};
use crate::ports::git::{TrainLanding, TrainStep};

/// The scratch checkout of `ci/` in which the merges are built.
const SCRATCH: &str = "candidate";

/// What the CI says of a playground task's head merged into the integration branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskChecks {
    /// No current check holds the merge: CI is off, absent, or ready to start its first run.
    Clear,
    /// Its run passed at this commit, whose tree the squash lands.
    Passed(String),
    /// Its run failed or was cancelled: merge on green only.
    Failed(CiRunRecord),
    /// Its run is queued, running or could not run (`Error`: not the code, run it again).
    Testing(CiRunRecord),
    /// The head conflicts with the integration branch in these paths.
    Conflict(Vec<String>),
}

impl TaskChecks {
    fn of_run(run: &CiRunRecord) -> Self {
        match run.state {
            RunState::Passed => Self::Passed(run.sha.clone()),
            RunState::Failed | RunState::Cancelled => Self::Failed(run.clone()),
            _ => Self::Testing(run.clone()),
        }
    }
}

impl App {
    /// CI configuration that makes the merge action build a candidate, knowing that the
    /// task head and integration base have no run yet.
    async fn playground_candidate_gate(
        &self,
        sandbox: &SandboxRecord,
        head: &str,
    ) -> Option<GateView> {
        let view = self.ci_gate(sandbox).await;
        if view.gate.is_some()
            || !self
                .ci_jobs_at(&self.sandbox_repo(&sandbox.id), head)
                .await
                .unwrap_or_default()
                .is_empty()
        {
            Some(view)
        } else {
            None
        }
    }

    /// Read-only verdict for the delivery view. An exact candidate already passed its merge
    /// step. Without one, preview the merge in the checkout the action will use.
    pub(crate) async fn playground_view_checks(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskRecord,
        head: &str,
        base: &str,
        worktree: &Path,
        diverged: bool,
    ) -> AppResult<TaskChecks> {
        let git = &self.deps.git;
        let repo = self.sandbox_repo(&sandbox.id);
        if self.config.ci.enabled && delivery_profile(sandbox).require_checks {
            let runs = self.deps.store.ci_runs(&sandbox.id, PLAN_RUNS).await?;
            if let Some(run) = self.task_run_of_head(task, &runs, head).await {
                return Ok(TaskChecks::of_run(run));
            }
            if diverged && let Some(view) = self.playground_candidate_gate(sandbox, head).await {
                // The CI train starts in a checkout of the task head and merges the base into it.
                match git.preview_merge(worktree, head, base).await? {
                    TrainStep::Conflict(files) => return Ok(TaskChecks::Conflict(files)),
                    TrainStep::Clean { tree } => {
                        // If only the task carried jobs, the merged tree can have lost them
                        // to a change on the base. The action then falls back to the raw squash.
                        if view.gate.is_some()
                            || self.ci_gate_for_head(sandbox, &view, &tree).await.is_some()
                        {
                            return Ok(TaskChecks::Clear);
                        }
                    }
                }
            }
        }
        if diverged && let TrainStep::Conflict(files) = git.preview_merge(&repo, base, head).await?
        {
            return Ok(TaskChecks::Conflict(files));
        }
        Ok(TaskChecks::Clear)
    }

    /// The checks of `task`'s `head` merged into its playground's integration branch as Yulius's
    /// clone has it now: the newest run of the task that tested exactly them, else a new merge
    /// built and queued (older active runs of the task cancelled as superseded). Callers hold the
    /// sandbox's armed-merge pass ([`App::armed_pass`]), so builds never share the scratch
    /// checkout.
    ///
    /// # Errors
    /// Git or store failures.
    pub(crate) async fn playground_task_checks(
        &self,
        sandbox: &SandboxRecord,
        task: &TaskRecord,
        head: &str,
    ) -> AppResult<TaskChecks> {
        if !self.config.ci.enabled {
            return Ok(TaskChecks::Clear);
        }
        let git = &self.deps.git;
        let repo = self.sandbox_repo(&sandbox.id);
        let integration = format!("origin/{}", delivery_profile(sandbox).integration_branch);
        let base = git
            .resolve_commit(&repo, &integration)
            .await?
            .ok_or_else(|| AppError::Conflict(format!("`{integration}` is missing")))?;
        let reference = CiRef::Task(u64::from(task.number));
        let runs: Vec<CiRunRecord> = self
            .deps
            .store
            .ci_runs(&sandbox.id, PLAN_RUNS)
            .await?
            .into_iter()
            .filter(|run| run.reference == reference)
            .collect();
        // Newest first: a run again of the same commit is its verdict.
        for run in &runs {
            if self.tests_exactly(&repo, &run.sha, head, &base).await {
                return Ok(TaskChecks::of_run(run));
            }
        }
        let Some(view) = self.playground_candidate_gate(sandbox, head).await else {
            return Ok(TaskChecks::Clear);
        };
        let ci_dir = self.ci_dir(&sandbox.id);
        // Readied through handles: Git checks out in it by its path (SPEC-367).
        self.ci_checkout_ready(&sandbox.id, &[], SCRATCH)
            .await
            .map_err(|err| {
                let at =
                    |reason: String| format!("readying the CI checkout `ci/{SCRATCH}`: {reason}");
                match err {
                    AppError::Forbidden(reason) => AppError::Forbidden(at(reason)),
                    AppError::Conflict(reason) => AppError::Conflict(at(reason)),
                    AppError::Internal(reason) => AppError::Internal(at(reason)),
                    other => other,
                }
            })?;
        let built = git
            .build_train(
                &repo,
                &ci_dir.join(SCRATCH),
                &ci_dir,
                &base,
                &[head.to_owned()],
                TrainLanding::MergeCommit,
            )
            .await?;
        match built.steps.first() {
            Some(TrainStep::Clean { .. }) => {}
            Some(TrainStep::Conflict(files)) => return Ok(TaskChecks::Conflict(files.clone())),
            None => return Err(AppError::internal("a merge was built without its head")),
        }
        let candidate = built.head;
        // The integration branch's jobs, with the task's own changed ones beside them, and its
        // own when the branch has none, as for a pull request (`ci_gate_for_head`, SPEC-504).
        let Some(gate) = self.ci_gate_for_head(sandbox, &view, &candidate).await else {
            return Ok(TaskChecks::Clear);
        };
        let reason = format!("superseded by {}", short(&candidate));
        for run in runs.iter().filter(|run| run.state.is_active()) {
            if let Err(err) = self.ci_cancel(&run.id, &reason).await {
                tracing::warn!(error = %err, run = %run.id, "superseding a task's CI run failed");
            }
        }
        let run = self
            .ci_queue(
                sandbox,
                reference,
                &candidate,
                &gate,
                &format!("task:{}", task.number),
            )
            .await?;
        Ok(TaskChecks::Testing(run))
    }

    /// Whether `commit` (a merge this module built) tests exactly `head` merged into `base`: its
    /// parents are `base` and either `head` (it contained `base`) or Yulius's merge of `base` into
    /// `head`, whose parents are exactly `head` and `base` (a later commit of the branch has one
    /// parent, or other ones).
    async fn tests_exactly(&self, repo: &Path, commit: &str, head: &str, base: &str) -> bool {
        let git = &self.deps.git;
        let Ok(Some(head)) = git.resolve_commit(repo, head).await else {
            return false;
        };
        let parents = git.parents(repo, commit).await.unwrap_or_default();
        let [first, merged] = parents.as_slice() else {
            return false;
        };
        if first != base {
            return false;
        }
        *merged == head
            || git
                .parents(repo, merged)
                .await
                .is_ok_and(|p| p.as_slice() == [head.clone(), base.to_owned()])
    }

    /// The newest run of a playground `task` among `runs` that tests `head` merged into its
    /// integration branch as Yulius's clone has it now (`yulius wait`, SPEC-163 PCI-006).
    pub(crate) async fn task_run_of_head<'r>(
        &self,
        task: &TaskRecord,
        runs: &'r [CiRunRecord],
        head: &str,
    ) -> Option<&'r CiRunRecord> {
        let reference = CiRef::Task(u64::from(task.number));
        if !runs.iter().any(|run| run.reference == reference) {
            return None;
        }
        let sandbox = self.deps.store.sandbox(&task.sandbox_id).await.ok()??;
        let repo = self.sandbox_repo(&sandbox.id);
        let integration = format!("origin/{}", delivery_profile(&sandbox).integration_branch);
        let base = self
            .deps
            .git
            .resolve_commit(&repo, &integration)
            .await
            .ok()??;
        for run in runs.iter().filter(|run| run.reference == reference) {
            if self.tests_exactly(&repo, &run.sha, head, &base).await {
                return Some(run);
            }
        }
        None
    }
}
