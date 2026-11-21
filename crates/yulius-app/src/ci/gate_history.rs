//! First-parent evidence for integration gate failures (SPEC-612).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use yulius_contracts::api::ci::GateNotice;
use yulius_domain::ci::RunState;
use yulius_domain::task::TaskStatus;

use crate::context::App;
use crate::delivery::delivery_profile;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;

/// The facts about a job needed after its console output expires.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateJob {
    /// Job name including the matrix cell.
    pub name: String,
    /// The job ran and passed.
    pub passed: bool,
    /// The names CI retained when the job failed.
    pub tests: Vec<String>,
    /// A cancelled sibling only observed part of its suite.
    pub partial: bool,
    /// A failing job whose evidence cannot identify every failure.
    pub unknown: bool,
}

/// The effective full gate on one first-parent revision.
pub struct GateVerdict {
    /// Judging run.
    pub run: String,
    /// Judged commit.
    pub sha: String,
    /// Verdict or active state.
    pub state: RunState,
    /// Merge owner's project task number and current status when still present.
    pub owner: Option<u32>,
    /// Whether that owner still has a task that can receive an outcome.
    pub status: Option<TaskStatus>,
    /// Only compact history remains for this run.
    pub archived: bool,
    /// Compact evidence of jobs that ran.
    pub jobs: Vec<GateJob>,
}

#[derive(Clone)]
struct Origin {
    sha: String,
    owner: Option<u32>,
    available: bool,
    run: Option<String>,
}

#[derive(Default)]
pub(super) struct GateFailures {
    tests: BTreeMap<String, (Origin, BTreeSet<String>)>,
    unknown: Vec<(String, Origin)>,
    latest_run: Option<String>,
    /// An earlier full suite still has to finish before attribution is final.
    pub waiting: bool,
}

impl GateFailures {
    fn observe(&mut self, verdict: GateVerdict) {
        if verdict.state.is_active() {
            self.waiting = true;
            return;
        }
        if verdict.state == RunState::Skipped {
            return;
        }
        if !verdict.archived {
            self.latest_run = Some(verdict.run.clone());
        }
        let origin = Origin {
            sha: verdict.sha,
            owner: verdict.owner,
            available: verdict.status.is_some_and(|status| !status.is_resting()),
            run: (!verdict.archived).then_some(verdict.run),
        };
        // A named failure can move between matrix cells. Compare all the job verdicts
        // together. Cancelled siblings and truncated failure lists cannot close a failure.
        let reported: BTreeSet<_> = verdict.jobs.iter().flat_map(|job| &job.tests).collect();
        let complete: BTreeSet<_> = verdict
            .jobs
            .iter()
            .filter(|job| !job.unknown && !job.partial)
            .map(|job| &job.name)
            .collect();
        self.tests.retain(|test, (_, jobs)| {
            if reported.contains(test) {
                return true;
            }
            jobs.retain(|job| !complete.contains(job));
            !jobs.is_empty()
        });
        self.unknown.clear();
        let mut observed = false;
        for job in verdict.jobs.into_iter().filter(|job| !job.passed) {
            observed = true;
            for test in job.tests {
                let (_, jobs) = self
                    .tests
                    .entry(test)
                    .or_insert_with(|| (origin.clone(), BTreeSet::new()));
                jobs.insert(job.name.clone());
            }
            if job.unknown {
                self.unknown.push((job.name, origin.clone()));
            }
        }
        if !observed {
            self.unknown
                .push((verdict.state.as_str().to_owned(), origin));
        }
    }

    /// Classify only failures actually observed at this revision.
    pub(super) fn describe(&self, verdict: &GateVerdict, requester: Option<u32>) -> (bool, String) {
        let mut inherited = Vec::new();
        let mut new = Vec::new();
        for job in &verdict.jobs {
            for test in &job.tests {
                match self.tests.get(test) {
                    Some((origin, _))
                        if origin.sha != verdict.sha
                            || (origin.owner.is_some() && origin.owner != requester) =>
                    {
                        let owner = origin
                            .owner
                            .map_or_else(|| "project".into(), |n| format!("task #{n}"));
                        inherited.push(format!(
                            "{test} from {} ({owner})",
                            super::staging::short(&origin.sha)
                        ));
                    }
                    _ => new.push(test.clone()),
                }
            }
            if job.unknown {
                new.push(job.name.clone());
            }
        }
        if new.is_empty() && inherited.is_empty() {
            new.push(verdict.state.as_str().to_owned());
        }
        let only_inherited = new.is_empty();
        let mut parts = Vec::new();
        if !new.is_empty() {
            parts.push(format!("new failures: {}", new.join(", ")));
        }
        if !inherited.is_empty() {
            parts.push(format!("inherited failures: {}", inherited.join(", ")));
        }
        (only_inherited, parts.join("; "))
    }

    fn notices(self) -> Vec<GateNotice> {
        self.tests
            .into_iter()
            .map(|(test, (origin, _))| (test, origin))
            .chain(self.unknown)
            .filter(|(_, origin)| !origin.available)
            .filter_map(|(test, origin)| {
                let run_id = origin.run.or_else(|| self.latest_run.clone())?;
                Some(GateNotice {
                    revision: origin.sha,
                    run_id,
                    test,
                })
            })
            .collect()
    }
}

impl App {
    /// Replay the branch's retained verdicts since its last full pass in first-parent order.
    pub(super) async fn gate_failures_at(
        &self,
        sandbox: &SandboxRecord,
        repo: &Path,
        head: &str,
        settled_only: bool,
    ) -> AppResult<GateFailures> {
        let branch = delivery_profile(sandbox).integration_branch;
        let search = async {
            let mut revisions = vec![head.to_owned()];
            let mut history = Vec::new();
            while let Some(oldest) = revisions.last() {
                let verdicts = self
                    .deps
                    .store
                    .integration_verdicts(&sandbox.id, &branch, &revisions, settled_only)
                    .await?;
                let mut passed = false;
                for verdict in verdicts {
                    if verdict.state == RunState::Passed {
                        passed = true;
                        break;
                    }
                    history.push(verdict);
                }
                if passed {
                    break;
                }
                revisions = self.deps.git.preceding_commits(repo, oldest).await?;
            }
            let mut failures = GateFailures::default();
            for verdict in history.into_iter().rev() {
                failures.observe(verdict);
            }
            Ok(failures)
        };
        tokio::time::timeout(Duration::from_secs(30), search)
            .await
            .map_err(|_| AppError::Conflict("reading integration gate history timed out".into()))?
    }

    /// Unresolved failures a project reader can act on even without a staging request.
    pub(crate) async fn project_gate_notices(
        &self,
        sandbox: &SandboxRecord,
    ) -> AppResult<Vec<GateNotice>> {
        let branch = delivery_profile(sandbox).integration_branch;
        if !self
            .deps
            .store
            .has_unowned_gate_failure(&sandbox.id, &branch)
            .await?
        {
            return Ok(Vec::new());
        }
        let repo = self.sandbox_repo(&sandbox.id);
        let Some(head) = self
            .deps
            .git
            .resolve_commit(&repo, &format!("refs/remotes/origin/{branch}"))
            .await?
        else {
            return Ok(Vec::new());
        };
        Ok(self
            .gate_failures_at(sandbox, &repo, &head, true)
            .await?
            .notices())
    }
}
