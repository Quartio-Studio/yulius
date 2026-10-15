//! Yulius CI of a playground's task (SPEC-163 PCI-002): a playground has no pull request, so its
//! task's head merged into the integration branch is built in Yulius's clone, as a merge train
//! builds its commit (SPEC-084), and tested as `task/<n>` before `delivery_merge` squashes it.
//! The build is a merge commit whose parents name what it tests: the integration head first, then
//! the head itself or Yulius's merge of the integration head into it (whose parents are the head
//! and the integration head). A head's run is the newest whose commit has exactly those parents, never one of a later
//! commit that contains it (review round 1). Once it passed, the squash lands that commit's tree,
//! so the integration head after it reuses the result.

use std::path::Path;

use yulius_domain::ci::{CiRef, Gate, GateSource, RunState};

use super::{CiRunRecord, PLAN_RUNS, own_jobs_apply, short};
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
    /// Nothing to wait for: the playground has no CI (or CI is off here).
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

impl App {
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
                return Ok(match run.state {
                    RunState::Passed => TaskChecks::Passed(run.sha.clone()),
                    RunState::Failed | RunState::Cancelled => TaskChecks::Failed(run.clone()),
                    _ => TaskChecks::Testing(run.clone()),
                });
            }
        }
        let view = self.ci_gate(sandbox).await;
        let at_head = self.ci_jobs_at(&repo, head).await.unwrap_or_default();
        if view.gate.is_none() && at_head.is_empty() {
            return Ok(TaskChecks::Clear);
        }
        let ci_dir = self.ci_dir(&sandbox.id);
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
        // Its own jobs when the task changed them, as for a pull request (CI-110), and when the
        // integration branch has none yet (bootstrap).
        let own = match self.ci_jobs_at(&repo, &candidate).await {
            Ok(jobs) if !jobs.is_empty() => Some(jobs),
            _ => None,
        };
        let at_base = match git.merge_base(&repo, head, &base).await {
            Ok(Some(since)) => self.ci_jobs_at(&repo, &since).await.ok(),
            _ => None,
        };
        let gate = match (view.gate, own) {
            (Some(gate), Some(jobs)) if own_jobs_apply(&jobs, at_base.as_deref(), &gate.jobs) => {
                Gate {
                    jobs,
                    source: GateSource::PullRequest,
                }
            }
            (Some(gate), _) => gate,
            (None, Some(jobs)) => Gate {
                jobs,
                source: GateSource::PullRequest,
            },
            (None, None) => return Ok(TaskChecks::Clear),
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
