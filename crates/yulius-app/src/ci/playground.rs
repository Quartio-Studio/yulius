//! Yulius CI of a playground's task (SPEC-163 PCI-002): a playground has no pull request, so its
//! task's head merged into the integration branch is built in Yulius's clone, as a merge train
//! builds its commit (SPEC-084), and tested as `task/<n>` before `delivery_merge` squashes it.
//! The build is a merge commit, so the commit contains the head (the run of a head is found by
//! ancestry) and has the tree the squash lands (the integration head after it reuses the result).

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
    /// Nothing to wait for: it passed, or the playground has no CI (or CI is off here).
    Clear,
    /// Its run failed or was cancelled: merge on green only.
    Failed(CiRunRecord),
    /// Its run is queued, running or could not run (not the code: run it again).
    Testing(RunState),
    /// The head conflicts with the integration branch in these paths.
    Conflict(Vec<String>),
}

impl App {
    /// The checks of `task`'s `head` merged into its playground's integration branch as Yulius's
    /// clone has it now: the newest run of the task whose commit contains both, else a new merge
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
            if git
                .is_ancestor(&repo, head, &run.sha)
                .await
                .unwrap_or(false)
                && git
                    .is_ancestor(&repo, &base, &run.sha)
                    .await
                    .unwrap_or(false)
            {
                return Ok(match run.state {
                    RunState::Passed => TaskChecks::Clear,
                    RunState::Failed | RunState::Cancelled => TaskChecks::Failed(run.clone()),
                    state => TaskChecks::Testing(state),
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
        Ok(TaskChecks::Testing(run.state))
    }

    /// The newest run of a playground `task` whose commit contains `head` (its head merged into
    /// the integration branch), among `runs`.
    pub(crate) async fn task_run_containing<'r>(
        &self,
        task: &TaskRecord,
        runs: &'r [CiRunRecord],
        head: &str,
    ) -> Option<&'r CiRunRecord> {
        let repo = self.sandbox_repo(&task.sandbox_id);
        let reference = CiRef::Task(u64::from(task.number));
        for run in runs.iter().filter(|run| run.reference == reference) {
            if self
                .deps
                .git
                .is_ancestor(&repo, head, &run.sha)
                .await
                .unwrap_or(false)
            {
                return Some(run);
            }
        }
        None
    }
}
