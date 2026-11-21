//! First-parent evidence for integration gate failures (SPEC-612).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::task_message::{Sender, Typed};
use serde::{Deserialize, Serialize};
use yulius_contracts::api::ci::GateNotice;
use yulius_domain::ci::RunState;
use yulius_domain::harness::Harness;
use yulius_domain::ids::TaskId;

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
    /// Merge owner's project task number.
    pub owner: Option<u32>,
    /// Only compact history remains for this run.
    pub archived: bool,
    /// Compact evidence of jobs that ran.
    pub jobs: Vec<GateJob>,
}

/// A worker yet to receive the current integration repair assignment.
#[derive(Debug, PartialEq, Eq)]
pub struct RepairRecipient {
    /// Running recipient.
    pub task: TaskId,
    /// Project number of the assigned worker.
    pub owner: u32,
    /// This recipient owns the repair.
    pub assigned: bool,
}

#[derive(Clone)]
struct Origin {
    sha: String,
    owner: Option<u32>,
    run: Option<String>,
}

#[derive(Default)]
pub(super) struct GateFailures {
    tests: BTreeMap<String, (Origin, BTreeSet<String>)>,
    unknown: BTreeMap<String, Origin>,
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
        self.unknown.retain(|job, _| !complete.contains(job));
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
                self.unknown
                    .entry(job.name)
                    .or_insert_with(|| origin.clone());
            }
        }
        if !observed {
            self.unknown
                .entry(verdict.state.as_str().to_owned())
                .or_insert(origin);
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
            .filter_map(|(test, origin)| {
                let run_id = self.latest_run.clone().or(origin.run)?;
                Some(GateNotice {
                    revision: origin.sha,
                    merge_task: origin.owner,
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
                if settled_only && verdict.state.is_active() {
                    // A preceding revision has no settled verdict yet. The next tick can
                    // attribute the newer failures once that revision is known.
                    break;
                }
                failures.observe(verdict);
            }
            Ok(failures)
        };
        tokio::time::timeout(Duration::from_secs(30), search)
            .await
            .map_err(|_| AppError::Conflict("reading integration gate history timed out".into()))?
    }

    /// Read the integration head independently of whether its task asked for staging.
    async fn integration_failures(&self, sandbox: &SandboxRecord) -> AppResult<Vec<GateNotice>> {
        let branch = delivery_profile(sandbox).integration_branch;
        if !self
            .deps
            .store
            .has_gate_failure(&sandbox.id, &branch)
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

    async fn available_gate_workers(&self, sandbox: &SandboxRecord) -> AppResult<Vec<TaskId>> {
        let candidates = self.deps.store.gate_workers(&sandbox.id).await?;
        if candidates.is_empty() {
            return Ok(candidates);
        }
        let held = tokio::time::timeout(
            super::staging::TASK_VIEW_BUDGET,
            self.browser_tab_requests(&sandbox.id),
        )
        .await
        .map_err(|_| {
            AppError::Conflict("reading repair workers' browser requests timed out".into())
        })?;
        let mut available = Vec::new();
        for id in candidates {
            if held.contains(id.as_str()) || self.parking(&id) {
                continue;
            }
            let Some(runtime) = self.existing_runtime(&id) else {
                continue;
            };
            if runtime.screen_withheld() || runtime.typing_opening.load(Ordering::SeqCst) {
                continue;
            }
            let Some(task) = self.deps.store.task(&id).await? else {
                continue;
            };
            if self.check_session_input_ready(&task, &runtime).is_ok()
                && !self.is_paused(&task).await
                && (self.box_held(&id).is_none() || self.held_draft_box_cleared(&task))
                && !(task.harness == Harness::ClaudeCode && runtime.hub.shows_draft())
            {
                available.push(id);
            }
        }
        Ok(available)
    }

    /// Unresolved failures visible to people when the project has no repair worker.
    pub(crate) async fn project_gate_notices(
        &self,
        sandbox: &SandboxRecord,
    ) -> AppResult<Vec<GateNotice>> {
        let failures = self.integration_failures(sandbox).await?;
        if failures.is_empty() || self.available_gate_workers(sandbox).await?.is_empty() {
            Ok(failures)
        } else {
            Ok(Vec::new())
        }
    }

    /// Tell the project's workers who repairs the integration branch. A held terminal or
    /// unavailable attempt leaves the assignment due for the next staging tick.
    ///
    /// # Errors
    /// An unreadable project, gate or assignment defers the notice to the next tick.
    pub async fn tell_gate_repairs(&self, sandbox: &SandboxRecord) -> AppResult<usize> {
        let store = &self.deps.store;
        let failures = self.integration_failures(sandbox).await?;
        let detail = failures
            .iter()
            .map(|n| {
                let origin = n
                    .merge_task
                    .map_or_else(|| "untracked merge".into(), |n| format!("task #{n}"));
                format!(
                    "{} at {} ({origin})",
                    n.test,
                    super::staging::short(&n.revision)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let available = if failures.is_empty() {
            Vec::new()
        } else {
            self.available_gate_workers(sandbox).await?
        };
        let recipients = store
            .reconcile_gate_repair(&sandbox.id, &detail, &available)
            .await?;
        if recipients.is_empty() || self.update_holds_starts() {
            return Ok(0);
        }
        let mut told = 0;
        for recipient in recipients {
            let Some(task) = store.task(&recipient.task).await? else {
                continue;
            };
            let Some(attempt) = store.live_attempt(&recipient.task).await? else {
                continue;
            };
            let assignment = if recipient.assigned {
                "You own this repair. Repair the integration branch before delivering your remaining work.".to_owned()
            } else {
                format!(
                    "Task #{} owns this repair. Continue your own work and leave the repair to that task.",
                    recipient.owner
                )
            };
            let message = self
                .redact_for_sandbox(
                    &sandbox.id,
                    &format!("Yulius: integration gate failed: {detail}. {assignment}"),
                )
                .await;
            let paste = format!(
                "\x1b[200~{}\x1b[201~",
                crate::task_message::clean_message(&message)
            );
            let admit = async {
                tokio::time::timeout(
                    super::staging::TASK_VIEW_BUDGET,
                    self.browser_tab_requests(&sandbox.id),
                )
                .await
                .is_ok_and(|held| !held.contains(task.id.as_str()))
                    && !self.update_holds_starts()
                    && !self.is_paused(&task).await
                    && store
                        .gate_repair_due(&recipient, &detail, false)
                        .await
                        .unwrap_or(false)
            };
            if self
                .type_into_session_if(
                    &task,
                    paste.as_bytes(),
                    Typed::Notice(&attempt.id),
                    Sender::Yulius,
                    Some(admit),
                )
                .await
                .is_ok()
                && store.gate_repair_due(&recipient, &detail, true).await?
            {
                told += 1;
            }
        }
        Ok(told)
    }
}
