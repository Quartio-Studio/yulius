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
use yulius_domain::staging_gate::{Gate, Point, judging};

use crate::context::App;
use crate::delivery::delivery_profile;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;

/// Select either the verdict that judges a request or observations for failure attribution.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GateRead {
    /// Latest admission, including a run in progress.
    Latest,
    /// Earlier completed attempts and the newest admission for each revision.
    History,
    /// Prefer completed evidence while a rerun is in progress.
    SettledHistory,
}

/// The facts about a job needed after its console output expires.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GateJob {
    /// Job name including the matrix cell.
    pub name: String,
    /// The job ran and passed.
    pub passed: bool,
    /// The names CI retained when the job failed.
    pub tests: Vec<String>,
    /// The gate omitted this job. Its preceding failures remain unresolved.
    #[serde(default)]
    pub skipped: bool,
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
    /// Compact evidence of jobs that ran.
    pub jobs: Vec<GateJob>,
}

impl GateVerdict {
    /// The verdict as the full gate's judging plan reads it (SPEC-653), with the failures
    /// `GateFailures::observe` attributes: a skipped job of a failed run is none of them. A
    /// cancelled or skipped run, and a pass that skipped an unresolved job, judge only their own
    /// request (its verdict path decides it), so they are none here.
    pub(crate) fn gate(&self) -> Option<Gate> {
        match self.state {
            RunState::Queued | RunState::Running => Some(Gate::Active),
            RunState::Passed if self.jobs.iter().any(|j| j.skipped) => None,
            RunState::Passed => Some(Gate::Passed),
            RunState::Failed | RunState::Error => {
                let mut failures = BTreeSet::new();
                for job in self.jobs.iter().filter(|job| !job.passed && !job.skipped) {
                    if job.unknown || job.tests.is_empty() {
                        failures.insert(format!("job {}", job.name));
                    }
                    failures.extend(job.tests.iter().cloned());
                }
                if failures.is_empty() {
                    failures.insert(format!("run {}", self.state.as_str()));
                }
                Some(Gate::Failed(failures))
            }
            RunState::Cancelled | RunState::Skipped => None,
        }
    }
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
}

#[derive(Default)]
pub(super) struct GateFailures {
    tests: BTreeMap<String, (Origin, BTreeSet<String>)>,
    unknown: BTreeMap<String, Origin>,
    /// A run-level verdict without failing job evidence lasts until another suite settles.
    run_failure: Option<(String, Origin)>,
    latest_run: Option<String>,
    /// An earlier full suite still has to finish before attribution is final.
    pub waiting: bool,
    /// Open failures kept only because later passing runs skipped their job: notices for
    /// people, but they stop no merge (SPEC-676).
    carried: BTreeSet<String>,
    /// Failures of settled red verdicts the replay did not reach (narrowed, or above an active
    /// verdict): unattributed, they stop the merge queue (SPEC-676 RGS-002).
    held: Vec<GateNotice>,
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
        self.run_failure = None;
        self.latest_run = Some(verdict.run);
        if verdict.state == RunState::Passed {
            let skipped: BTreeSet<_> = verdict
                .jobs
                .iter()
                .filter(|j| j.skipped)
                .map(|j| &j.name)
                .collect();
            self.unknown.retain(|job, _| skipped.contains(job));
            self.tests.retain(|_, (_, jobs)| {
                jobs.retain(|job| skipped.contains(job));
                !jobs.is_empty()
            });
            self.carried.extend(self.tests.keys().cloned());
            self.carried.extend(self.unknown.keys().cloned());
            return;
        }
        let origin = Origin {
            sha: verdict.sha,
            owner: verdict.owner,
        };
        // A named failure can move between matrix cells. Compare all the job verdicts
        // together. Cancelled siblings and truncated failure lists cannot close a failure.
        let reported: BTreeSet<_> = verdict
            .jobs
            .iter()
            .filter(|job| !job.passed && !job.skipped)
            .flat_map(|job| &job.tests)
            .collect();
        let complete: BTreeSet<_> = verdict
            .jobs
            .iter()
            .filter(|job| !job.unknown && !job.partial && !job.skipped)
            .map(|job| &job.name)
            .collect();
        self.tests.retain(|test, (_, jobs)| {
            jobs.retain(|job| !complete.contains(job));
            reported.contains(test) || !jobs.is_empty()
        });
        self.unknown.retain(|job, _| !complete.contains(job));
        let mut observed = false;
        for job in verdict
            .jobs
            .into_iter()
            .filter(|job| !job.passed && !job.skipped)
        {
            observed = true;
            if job.unknown {
                self.carried.remove(&job.name);
            }
            for test in job.tests {
                self.carried.remove(&test);
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
            self.run_failure = Some((verdict.state.as_str().to_owned(), origin));
        }
    }

    /// Describe every still-open failure. A job that failed here without identifying its tests
    /// gives no evidence that its cause is the same as an earlier job failure.
    pub(super) fn describe(
        &self,
        verdict: &GateVerdict,
        requester: Option<u32>,
    ) -> Option<(bool, String)> {
        let mut inherited = Vec::new();
        let mut new = Vec::new();
        let mut classify = |name: &str, origin: Option<&Origin>| {
            if let Some(origin) = origin
                .filter(|o| o.sha != verdict.sha || (o.owner.is_some() && o.owner != requester))
            {
                let owner = origin
                    .owner
                    .map_or_else(|| "project".into(), |n| format!("task #{n}"));
                inherited.push(format!(
                    "{name} from {} ({owner})",
                    super::staging::short(&origin.sha)
                ));
            } else {
                new.push(name.to_owned());
            }
        };
        for (test, (origin, _)) in &self.tests {
            classify(test, Some(origin));
        }
        let unidentified: BTreeSet<&str> = verdict
            .jobs
            .iter()
            .filter(|job| job.unknown)
            .map(|job| job.name.as_str())
            .collect();
        if let Some((state, _)) = &self.run_failure {
            classify(state, None);
        }
        for (job, origin) in &self.unknown {
            classify(
                job,
                (!unidentified.contains(job.as_str())).then_some(origin),
            );
        }
        if new.is_empty() && inherited.is_empty() {
            return None;
        }
        let only_inherited = new.is_empty();
        let mut parts = Vec::new();
        if !new.is_empty() {
            parts.push(format!("new failures: {}", new.join(", ")));
        }
        if !inherited.is_empty() {
            parts.push(format!("inherited failures: {}", inherited.join(", ")));
        }
        Some((only_inherited, parts.join("; ")))
    }

    /// Keeps the failures of `verdict`, a settled red verdict the replay did not reach, for the
    /// merge queue's stop (SPEC-676 RGS-002).
    fn hold(&mut self, verdict: &GateVerdict) {
        if verdict.state.is_active() {
            return;
        }
        if let Some(Gate::Failed(failures)) = verdict.gate() {
            for test in failures {
                self.held.push(GateNotice {
                    key: crate::notice_producers::gate_key(&test).to_string(),
                    revision: verdict.sha.clone(),
                    merge_task: None,
                    run_id: verdict.run.clone(),
                    test,
                });
            }
        }
    }

    /// The failures that make the integration branch red for the merge queue (SPEC-676
    /// RGS-002): the open ones but those only skipped since, and those of red verdicts not yet
    /// attributed.
    pub(super) fn red(self) -> Vec<GateNotice> {
        let carried = self.carried.clone();
        let held = self.held.clone();
        let mut red: Vec<GateNotice> = self
            .notices()
            .into_iter()
            .filter(|notice| !carried.contains(&notice.test))
            .collect();
        for notice in held {
            if !red.iter().any(|known| known.test == notice.test) {
                red.push(notice);
            }
        }
        red
    }

    fn notices(self) -> Vec<GateNotice> {
        let Some(run_id) = self.latest_run else {
            return Vec::new();
        };
        self.tests
            .into_iter()
            .map(|(test, (origin, _))| (test, origin))
            .chain(self.unknown)
            .chain(self.run_failure)
            .map(|(test, origin)| GateNotice {
                key: crate::notice_producers::gate_key(&test).to_string(),
                revision: origin.sha,
                merge_task: origin.owner,
                run_id: run_id.clone(),
                test,
            })
            .collect()
    }
}

/// The red revisions among `walked` (first-parent, newest first) whose range still holds a
/// revision of `asked` that nothing judged (SPEC-653 NWF-004); `history` holds their verdicts,
/// newest admission first, and `green` the full pass the walk stopped at.
fn unresolved_ranges(
    walked: &[String],
    history: &[GateVerdict],
    green: Option<&str>,
    asked: &[String],
) -> BTreeSet<String> {
    if asked.is_empty() {
        return BTreeSet::new();
    }
    let count = walked.len() as u64;
    let points: Vec<Point> = walked
        .iter()
        .zip((1..=count).rev())
        .map(|(sha, position)| Point {
            position,
            gate: if green == Some(sha.as_str()) {
                Some(Gate::Passed)
            } else {
                history
                    .iter()
                    .find(|v| &v.sha == sha)
                    .and_then(GateVerdict::gate)
            },
            asked: asked.contains(sha),
        })
        .collect();
    judging(&points)
        .unresolved
        .into_iter()
        .filter_map(|at| walked.get(usize::try_from(count - at).ok()?).cloned())
        .collect()
}

/// Replays `history` (verdicts since the last full pass, newest first) oldest first, up to a
/// revision whose range is still `unresolved` or, when `settled_only`, an active verdict; the
/// red verdicts after that point are held for the merge queue's stop (SPEC-676).
fn replay(
    history: Vec<GateVerdict>,
    unresolved: &BTreeSet<String>,
    settled_only: bool,
) -> GateFailures {
    let mut failures = GateFailures::default();
    let mut oldest_first = history.into_iter().rev();
    while let Some(verdict) = oldest_first.next() {
        if unresolved.contains(&verdict.sha) {
            failures.waiting = true;
            failures.hold(&verdict);
            break;
        }
        if settled_only && verdict.state.is_active() {
            // A preceding revision has no settled verdict yet. The next tick can
            // attribute the newer failures once that revision is known.
            break;
        }
        failures.observe(verdict);
    }
    for verdict in oldest_first {
        failures.hold(&verdict);
    }
    failures
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
            let read = if settled_only {
                GateRead::SettledHistory
            } else {
                GateRead::History
            };
            let asked = self.staging_testing_revisions(sandbox).await?;
            let mut revisions = vec![head.to_owned()];
            let mut walked = Vec::new();
            let mut history = Vec::new();
            let mut green = None;
            while let Some(oldest) = revisions.last() {
                let verdicts = self
                    .deps
                    .store
                    .integration_verdicts(&sandbox.id, &branch, &revisions, read)
                    .await?;
                walked.extend(revisions.iter().cloned());
                for verdict in verdicts {
                    if verdict.state == RunState::Passed && !verdict.jobs.iter().any(|j| j.skipped)
                    {
                        green = Some(verdict.sha);
                        break;
                    }
                    history.push(verdict);
                }
                if green.is_some() {
                    break;
                }
                revisions = self.deps.git.preceding_commits(repo, oldest).await?;
            }
            // A failure whose range still holds a waiting staging request is not attributed
            // before that request is judged (SPEC-653 NWF-004).
            let unresolved = unresolved_ranges(&walked, &history, green.as_deref(), &asked);
            Ok(replay(history, &unresolved, settled_only))
        };
        tokio::time::timeout(Duration::from_secs(30), search)
            .await
            .map_err(|_| AppError::Conflict("reading integration gate history timed out".into()))?
    }

    /// Read the integration head independently of whether its task asked for staging.
    async fn integration_failures(&self, sandbox: &SandboxRecord) -> AppResult<Vec<GateNotice>> {
        Ok(self
            .integration_replay(sandbox)
            .await?
            .map(GateFailures::notices)
            .unwrap_or_default())
    }

    /// What makes the integration branch red for the merge queue (SPEC-676 RGS-002): its open
    /// failures but those only skipped since, and those of red verdicts not yet attributed.
    pub(crate) async fn integration_red(
        &self,
        sandbox: &SandboxRecord,
    ) -> AppResult<Vec<GateNotice>> {
        Ok(self
            .integration_replay(sandbox)
            .await?
            .map(GateFailures::red)
            .unwrap_or_default())
    }

    async fn integration_replay(&self, sandbox: &SandboxRecord) -> AppResult<Option<GateFailures>> {
        let branch = delivery_profile(sandbox).integration_branch;
        if !self
            .deps
            .store
            .has_gate_failure(&sandbox.id, &branch)
            .await?
        {
            return Ok(None);
        }
        let repo = self.sandbox_repo(&sandbox.id);
        let Some(head) = self
            .deps
            .git
            .resolve_commit(&repo, &format!("refs/remotes/origin/{branch}"))
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(
            self.gate_failures_at(sandbox, &repo, &head, true).await?,
        ))
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
            if runtime.screen_withheld()
                || runtime.typing_opening.load(Ordering::SeqCst)
                || !tokio::time::timeout(super::staging::TASK_VIEW_BUDGET, runtime.has_live_run())
                    .await
                    .unwrap_or(false)
            {
                continue;
            }
            let Some(task) = self.deps.store.task(&id).await? else {
                continue;
            };
            if self.accepts_machine_notice(&id).await?
                && self.check_session_input_ready(&task, &runtime).is_ok()
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
        // The project's people are told of what nobody repairs, one keyed notice per failure,
        // removed once it is repaired or owned (SPEC-624 NTC-010). "Nobody" is the stored state:
        // no task works in the project. A worker only momentarily unable to take a notice (a
        // draft, typing, no session yet) still owns it, so the notice does not flap (review of
        // #1711).
        // A worker that stays unable for the grace period no longer hides it (review round 2
        // of #1711).
        let unavailable_for = {
            let red_and_stuck = !failures.is_empty() && available.is_empty();
            let mut ticks = self
                .gate_unavailable_ticks
                .lock()
                .map_err(|_| AppError::internal("gate tick counts poisoned"))?;
            if red_and_stuck {
                let count = ticks.entry(sandbox.id.clone()).or_default();
                *count = count.saturating_add(1);
                *count
            } else {
                ticks.remove(&sandbox.id);
                0
            }
        };
        let (unowned, hold_new): (&[GateNotice], bool) =
            if failures.is_empty() || !available.is_empty() {
                (&[], false)
            } else {
                let past = unavailable_for >= crate::notice_producers::GATE_GRACE_TICKS
                    || store.gate_workers(&sandbox.id).await?.is_empty();
                (&failures, !past)
            };
        if let Err(err) = self.sync_gate_notices(sandbox, unowned, hold_new).await {
            tracing::warn!(sandbox = %sandbox.slug, error = %err, "gate notices could not be reconciled");
        }
        // The task whose merge brought a failure in repairs it first (SPEC-676 RGS-009).
        let breakers: Vec<u32> = failures.iter().filter_map(|n| n.merge_task).collect();
        let recipients = store
            .reconcile_gate_repair(&sandbox.id, &detail, &available, &breakers)
            .await?;
        if recipients.is_empty() || self.update_holds_starts() {
            return Ok(0);
        }
        // While the red gate stops the merge queue, every worker hears that merges wait.
        let stopped = self.merge_stop(sandbox).await.is_some();
        let mut told = 0;
        for recipient in recipients {
            let Some(task) = store.task(&recipient.task).await? else {
                continue;
            };
            let Some(attempt) = store.live_attempt(&recipient.task).await? else {
                continue;
            };
            let mut assignment = if recipient.assigned {
                "You own this repair. Repair the integration branch before delivering your remaining work.".to_owned()
            } else {
                format!(
                    "Task #{} owns this repair. Continue your own work and leave the repair to that task.",
                    recipient.owner
                )
            };
            if stopped {
                assignment.push_str(if recipient.assigned {
                    " The merge queue merges only the repair until the gate is green: deliver it with `delivery_merge` and `repairs_gate: true`."
                } else {
                    " Your merges wait until the gate is green; an armed one merges by itself then."
                });
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn job(name: &str, passed: bool, tests: &[&str]) -> GateJob {
        GateJob {
            name: name.to_owned(),
            passed,
            tests: tests.iter().map(|t| (*t).to_owned()).collect(),
            skipped: false,
            partial: false,
            unknown: false,
        }
    }

    fn verdict(sha: &str, state: RunState, jobs: Vec<GateJob>) -> GateVerdict {
        GateVerdict {
            run: format!("run-{sha}"),
            sha: sha.to_owned(),
            state,
            owner: None,
            jobs,
        }
    }

    fn failed(keys: &[&str]) -> Option<Gate> {
        Some(Gate::Failed(keys.iter().map(|k| (*k).to_owned()).collect()))
    }

    /// SPEC-653 (review round 1): the plan reads the failures attribution reads.
    #[test]
    fn a_verdict_names_the_failures_attribution_reads() {
        let tests = verdict(
            "a",
            RunState::Failed,
            vec![job("quick", true, &[]), job("full", false, &["t1", "t2"])],
        );
        assert_eq!(tests.gate(), failed(&["t1", "t2"]));
        let mut unknown = job("e2e", false, &["t3"]);
        unknown.unknown = true;
        let mut skipped = job("full", false, &[]);
        skipped.skipped = true;
        let mixed = verdict("b", RunState::Failed, vec![unknown, skipped.clone()]);
        assert_eq!(
            mixed.gate(),
            failed(&["job e2e", "t3"]),
            "a skipped job is no failure"
        );
        assert_eq!(
            verdict("c", RunState::Failed, vec![job("full", false, &[])]).gate(),
            failed(&["job full"])
        );
        assert_eq!(
            verdict("d", RunState::Error, Vec::new()).gate(),
            failed(&["run error"])
        );
        assert_eq!(
            verdict("e", RunState::Passed, Vec::new()).gate(),
            Some(Gate::Passed)
        );
        assert_eq!(
            verdict("f", RunState::Passed, vec![skipped]).gate(),
            None,
            "a pass that skipped an unresolved job judges only its own request"
        );
        for state in [RunState::Queued, RunState::Running] {
            assert_eq!(verdict("g", state, Vec::new()).gate(), Some(Gate::Active));
        }
        for state in [RunState::Cancelled, RunState::Skipped] {
            assert_eq!(verdict("h", state, Vec::new()).gate(), None);
        }
    }

    fn owned(mut verdict: GateVerdict, owner: u32) -> GateVerdict {
        verdict.owner = Some(owner);
        verdict
    }

    fn red_tests(failures: GateFailures) -> Vec<(String, Option<u32>, String)> {
        failures
            .red()
            .into_iter()
            .map(|n| (n.test, n.merge_task, n.run_id))
            .collect()
    }

    /// SPEC-676 RGS-002 (review round 1): a settled red verdict stops the queue while its range
    /// is still narrowed, or while an older verdict is active, unattributed; the notices for
    /// people still wait for the attribution.
    #[test]
    fn a_red_verdict_not_yet_attributed_still_stops_the_queue() {
        let red = |sha: &str| verdict(sha, RunState::Failed, vec![job("full", false, &["t"])]);
        let narrowing = replay(vec![red("c")], &BTreeSet::from(["c".to_owned()]), true);
        assert!(narrowing.waiting);
        assert!(narrowing.notices().is_empty(), "nobody is named yet");
        let narrowing = replay(vec![red("c")], &BTreeSet::from(["c".to_owned()]), true);
        assert_eq!(
            red_tests(narrowing),
            [("t".to_owned(), None, "run-c".to_owned())]
        );
        let active = verdict("a", RunState::Running, Vec::new());
        let behind = replay(vec![red("c"), active], &BTreeSet::new(), true);
        assert_eq!(
            red_tests(behind),
            [("t".to_owned(), None, "run-c".to_owned())]
        );
        let attributed = replay(vec![owned(red("c"), 4)], &BTreeSet::new(), true);
        assert_eq!(
            red_tests(attributed),
            [("t".to_owned(), Some(4), "run-c".to_owned())]
        );
    }

    /// SPEC-676 RGS-002 (review round 1): a failure kept open only because later runs skipped
    /// its job is still a notice, but stops nothing, until a run of its job fails it again.
    #[test]
    fn a_failure_only_skipped_since_stops_nothing() {
        let red = |sha: &str| verdict(sha, RunState::Failed, vec![job("full", false, &["t"])]);
        let mut skipped = job("full", true, &[]);
        skipped.skipped = true;
        let skipping = verdict(
            "s",
            RunState::Passed,
            vec![job("quick", true, &[]), skipped],
        );
        let carried = replay(vec![skipping, red("r")], &BTreeSet::new(), true);
        assert_eq!(carried.notices().len(), 1, "people still see it");
        let carried = replay(
            vec![
                verdict(
                    "s",
                    RunState::Passed,
                    vec![job("quick", true, &[]), {
                        let mut j = job("full", true, &[]);
                        j.skipped = true;
                        j
                    }],
                ),
                red("r"),
            ],
            &BTreeSet::new(),
            true,
        );
        assert!(carried.red().is_empty());
        let mut skipped = job("full", true, &[]);
        skipped.skipped = true;
        let again = replay(
            vec![
                red("n"),
                verdict("s", RunState::Passed, vec![skipped]),
                red("r"),
            ],
            &BTreeSet::new(),
            true,
        );
        assert_eq!(red_tests(again).len(), 1, "failed again where it ran");
    }

    /// SPEC-653 NWF-004: the walk's red revisions with a waiting request below them, down to the
    /// green revision the walk stopped at, by commit.
    #[test]
    fn unresolved_ranges_name_the_red_revision_above_a_waiting_request() {
        let walked: Vec<String> = ["h", "r", "w", "g", "x"].map(String::from).to_vec();
        let red = |sha: &str, test: &str| {
            verdict(sha, RunState::Failed, vec![job("full", false, &[test])])
        };
        let history = vec![red("h", "t"), red("r", "t")];
        let asked = vec!["w".to_owned()];
        assert_eq!(
            unresolved_ranges(&walked, &history, Some("g"), &asked),
            BTreeSet::from(["r".to_owned()])
        );
        assert!(unresolved_ranges(&walked, &history, Some("g"), &[]).is_empty());
        let judged = vec![red("h", "t"), red("r", "t"), red("w", "t")];
        assert!(
            unresolved_ranges(&walked, &judged, Some("g"), &asked).is_empty(),
            "the waiting revision has its verdict"
        );
        let old = vec![red("h", "t"), red("r", "t"), red("g", "t")];
        assert!(
            unresolved_ranges(&walked, &old, None, &asked).is_empty(),
            "r brings nothing g did not have"
        );
    }
}
