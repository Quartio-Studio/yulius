//! Yulius-run CI (SPEC-001 DLV-006, SPEC-002 ADR-21) and repository hygiene (DLV-007).
//!
//! Yulius is the projects' only CI (F 2026-10-10: no GitHub Actions; "local blacksmith
//! instance"). For every sandbox with CI configured — the profile's `gate`, else `[[ci.jobs]]`
//! or `[commands] gate` of `yulius.toml` on the integration branch — it tests each open pull
//! request head into the integration branch and the integration and production heads, once per
//! commit, and publishes the commit status `yulius/ci`. A pull request into an integration
//! branch without CI is tested with its own configuration (bootstrap, so the onboarding pull
//! request that replaces GitHub Actions is tested by what it adds).
//!
//! Ownership and bounds (RUN-001..003):
//! - the **sync loop** (every [`CiSettings::poll`]) fetches each sandbox clone, lists open pull
//!   requests, plans with [`yulius_domain::ci::plan`]: queues new heads and cancels runs whose
//!   reference moved (superseded) or disappeared;
//! - the **dispatcher** starts the oldest queued run of each sandbox that has none executing
//!   (up to [`CiSettings::lanes`] runs per sandbox at a time, each lane with its own persistent
//!   job checkouts);
//! - a run's jobs run in parallel, each holding one of [`CiSettings::max_jobs`] instance-wide
//!   slots; a job runs through the process host ([`crate::ports::session::SessionHost`]) inside
//!   the sandbox's containment, with no secret in its environment (only the placeholders and
//!   egress proxy of the agent secrets its integration branch entry grants, SPEC-142), for at most
//!   [`CiSettings::job_timeout`]; cancellation and timeouts kill the job's process group, and
//!   the host is terminated when the exit is not seen within [`KILL_GRACE`];
//! - output is the job's log file, read while it runs, kept as the last [`OUTPUT_BYTES`]
//!   (escape sequences removed); a log beyond [`MAX_LOG_FILE_BYTES`] ends the job;
//! - retention: [`KEEP_RUNS`] runs per sandbox and [`RETAIN_MS`]; commit statuses are retried
//!   [`STATUS_MAX_ATTEMPTS`] times within a day.
//!
//! Restart: runs recorded as running lose their executor; their hosts are terminated and they
//! go back to the queue (a run is repeatable).

pub mod cd;
mod disk_wait;
mod exec;
pub mod hygiene;
mod tools;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::{Notify, Semaphore, watch};
use yulius_contracts::api::ci::{CiGate, CiJob, CiJobSpec, CiOverview, CiRun};
use yulius_domain::ci::{
    self as dom, CiRef, Gate, GateResult, GateSource, Job, RunFacts, RunState, RunSummary, Target,
};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::resources::Need;
use yulius_domain::spec_format::{MAX_CONFIG_BYTES, parse_yulius_toml};
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::delivery::{delivery_profile, profile_gate};
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::github::{NewCommitStatus, STATUS_PERMISSION_MISSING};

/// Output kept per job (the tail).
pub const OUTPUT_BYTES: usize = 512 * 1024;
/// A job whose log grows beyond this is ended (runaway output would fill the disk).
pub const MAX_LOG_FILE_BYTES: u64 = 256 * 1024 * 1024;
/// Runs listed per sandbox.
pub const RUN_LIST_LIMIT: u32 = 30;
/// Runs kept per sandbox.
pub const KEEP_RUNS: u32 = 50;
/// Runs older than this are deleted.
pub const RETAIN_MS: i64 = 14 * 24 * 60 * 60 * 1000;
/// Attempts to publish one commit status.
pub const STATUS_MAX_ATTEMPTS: u32 = 5;
/// Open pull requests tested per sandbox.
pub const MAX_PULLS_TESTED: usize = 30;
/// Time to see a killed job's exit before its host is terminated.
pub const KILL_GRACE: Duration = Duration::from_secs(10);
/// GitHub Actions workflow files listed.
pub const MAX_WORKFLOW_FILES: usize = 20;
/// Where workflows live.
pub const WORKFLOWS_DIR: &str = ".github/workflows";
const GATE_CACHE_MS: i64 = 60_000;
const STATUS_RETRY_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;
const PLAN_RUNS: u32 = 200;
const DISPATCH_TICK: Duration = Duration::from_secs(15);
const PRUNE_EVERY_MS: i64 = 60 * 60 * 1000;

/// CI settings of the instance (`yulius serve` flags; SPEC-002 §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiSettings {
    /// Run CI at all.
    pub enabled: bool,
    /// Jobs running at once on this machine (all sandboxes).
    pub max_jobs: u32,
    /// Runs of one sandbox executing at once, each in its own job checkouts (F 2026-10-12: one
    /// run at a time made parallel tasks wait behind each other for hours).
    pub lanes: u32,
    /// Wall-clock limit of one job.
    pub job_timeout: Duration,
    /// How often each sandbox is checked for new heads.
    pub poll: Duration,
}

impl Default for CiSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            max_jobs: 4,
            lanes: 2,
            job_timeout: Duration::from_secs(30 * 60),
            poll: Duration::from_secs(60),
        }
    }
}

/// One CI run (one commit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiRunRecord {
    /// Id.
    pub id: String,
    /// Sandbox.
    pub sandbox_id: SandboxId,
    /// What it tests.
    pub reference: CiRef,
    /// Commit.
    pub sha: String,
    /// Where its configuration came from.
    pub source: GateSource,
    /// State (from its jobs).
    pub state: RunState,
    /// Why it errored or was cancelled.
    pub reason: Option<String>,
    /// `yulius` or a login / `task:<n>`.
    pub requested_by: String,
    /// The run state whose commit status GitHub last accepted.
    pub status_for: Option<RunState>,
    /// Why the last publication failed.
    pub status_note: Option<String>,
    /// Publication attempts.
    pub status_attempts: u32,
    /// Queued at.
    pub queued_at: Timestamp,
    /// Started at.
    pub started_at: Option<Timestamp>,
    /// Ended at.
    pub ended_at: Option<Timestamp>,
}

/// One job of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CiJobRecord {
    /// Id.
    pub id: String,
    /// Run.
    pub run_id: String,
    /// Position.
    pub ordinal: u32,
    /// Display name.
    pub name: String,
    /// Definition name (checkout and cache).
    pub key: String,
    /// Shell text.
    pub command: String,
    /// Matrix value.
    pub matrix: Option<String>,
    /// State.
    pub state: RunState,
    /// Why it errored or was cancelled.
    pub reason: Option<String>,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Output tail (empty unless read with outputs).
    pub output: String,
    /// Its failing tests' output, kept from its whole output (SPEC-145; read with outputs).
    pub failures: Option<String>,
    /// Started at.
    pub started_at: Option<Timestamp>,
    /// Ended at.
    pub ended_at: Option<Timestamp>,
    /// The CPUs and memory it declared (SPEC-082 RES-002).
    pub need: Need,
}

/// Fields set with a run transition.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CiRunUpdate {
    /// Reason.
    pub reason: Option<String>,
    /// Start.
    pub started_at: Option<Timestamp>,
    /// End.
    pub ended_at: Option<Timestamp>,
}

/// Fields set with a job transition.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CiJobUpdate {
    /// Reason.
    pub reason: Option<String>,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Output.
    pub output: Option<String>,
    /// Its failing tests' output (SPEC-145).
    pub failures: Option<String>,
    /// Start.
    pub started_at: Option<Timestamp>,
    /// End.
    pub ended_at: Option<Timestamp>,
}

/// The CI configuration of a sandbox as last read.
#[derive(Debug, Clone, Default)]
pub(crate) struct GateView {
    read_at: i64,
    base_sha: Option<String>,
    /// The CI in force, if any.
    pub(crate) gate: Option<Gate>,
    /// Why the integration branch's configuration is unusable.
    pub(crate) problem: Option<String>,
    /// GitHub Actions workflow files on the integration branch.
    pub(crate) workflows: Vec<String>,
}

/// Process-local CI state.
#[derive(Default)]
pub struct CiState {
    slots: OnceLock<Arc<Semaphore>>,
    /// The CPUs and MiB of memory CI jobs share, sized from the sandbox capacity at the first
    /// job (SPEC-082 RES-003).
    pub(crate) budget: OnceLock<crate::resources::CiPools>,
    /// Executing runs: their cancellation (the reason, once cancelled).
    executing: Mutex<HashMap<String, watch::Sender<Option<String>>>>,
    /// Lanes in use (`<sandbox>/<lane>` for an executing run) and whole-sandbox claims
    /// (`<sandbox>`, disk upkeep).
    busy: Mutex<HashSet<String>>,
    gates: Mutex<HashMap<String, GateView>>,
    pub(crate) hygiene: Mutex<HashMap<String, yulius_contracts::api::ci::HygieneReport>>,
    wake: Notify,
    started: AtomicBool,
    last_prune: Mutex<i64>,
    /// Sandboxes whose runs wait for disk (SPEC-059 CI-301).
    disk_waits: Mutex<HashMap<String, disk_wait::DiskWait>>,
    /// One pass deciding a sandbox's armed merges (and its merge train) at a time (SPEC-084
    /// TRN-010): passes started by runs that end together wait for each other.
    pub(crate) armed_passes: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// Whether a pull request is tested with its own jobs (`own`) instead of the integration branch's:
/// only when it changed them itself, against what it branched off (`at_base`, unknown: assumed
/// changed), and they differ from the integration branch's.
pub(crate) fn own_jobs_apply<J: PartialEq>(
    own: &[J],
    at_base: Option<&[J]>,
    integration: &[J],
) -> bool {
    own != integration && at_base.is_none_or(|base| own != base)
}

/// Whether anything of `sandbox` is in `busy`: a lane (`<sandbox>/<n>`) or a claim.
fn busy_in(busy: &HashSet<String>, sandbox: &str) -> bool {
    busy.iter().any(|key| {
        key == sandbox
            || key
                .strip_prefix(sandbox)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// A free lane of `sandbox`, unless the sandbox is claimed or every lane runs.
fn free_lane(busy: &HashSet<String>, sandbox: &str, lanes: u32) -> Option<u32> {
    if busy.contains(sandbox) {
        return None;
    }
    (0..lanes.max(1)).find(|lane| !busy.contains(&format!("{sandbox}/{lane}")))
}

/// [`free_lane`], and marks it.
fn take_lane(busy: &mut HashSet<String>, sandbox: &str, lanes: u32) -> Option<u32> {
    let lane = free_lane(busy, sandbox, lanes)?;
    busy.insert(format!("{sandbox}/{lane}"));
    Some(lane)
}

fn job_view(job: &CiJobRecord, with_output: bool) -> CiJob {
    CiJob {
        id: job.id.clone(),
        name: job.name.clone(),
        command: job.command.clone(),
        matrix: job.matrix.clone(),
        state: job.state.as_str().to_owned(),
        reason: job.reason.clone(),
        exit_code: job.exit_code,
        duration_ms: job
            .started_at
            .zip(job.ended_at)
            .map(|(start, end)| end.millis_since(start)),
        output: with_output.then(|| job.output.clone()),
        failures: with_output.then(|| job.failures.clone()).flatten(),
    }
}

fn status_word(run: &CiRunRecord) -> String {
    if run.status_for == Some(run.state) {
        "published".to_owned()
    } else if let Some(note) = &run.status_note {
        note.clone()
    } else {
        "pending".to_owned()
    }
}

/// The API form of a run.
#[must_use]
pub fn run_view(run: &CiRunRecord, jobs: &[CiJobRecord], with_output: bool) -> CiRun {
    CiRun {
        id: run.id.clone(),
        reference: run.reference.key(),
        label: run.reference.label(),
        pr_number: match run.reference {
            CiRef::PullRequest(n) => Some(n),
            CiRef::Branch(_) | CiRef::Train(_) => None,
        },
        sha: run.sha.clone(),
        state: run.state.as_str().to_owned(),
        source: run.source.as_str().to_owned(),
        jobs: jobs.iter().map(|job| job_view(job, with_output)).collect(),
        reason: run.reason.clone(),
        queued_at: rfc3339(run.queued_at),
        started_at: run.started_at.map(rfc3339),
        ended_at: run.ended_at.map(rfc3339),
        duration_ms: run
            .started_at
            .zip(run.ended_at)
            .map(|(start, end)| end.millis_since(start)),
        requested_by: run.requested_by.clone(),
        status: status_word(run),
    }
}

fn gate_view(gate: &Gate) -> CiGate {
    CiGate {
        jobs: gate
            .jobs
            .iter()
            .map(|job| CiJobSpec {
                name: job.name.clone(),
                command: job.command.clone(),
                matrix: job.matrix.clone(),
            })
            .collect(),
        source: gate.source.as_str().to_owned(),
    }
}

impl App {
    fn ci_slots(&self) -> Arc<Semaphore> {
        self.ci
            .slots
            .get_or_init(|| {
                let jobs = usize::try_from(self.config.ci.max_jobs.max(1)).unwrap_or(1);
                Arc::new(Semaphore::new(jobs))
            })
            .clone()
    }

    pub(crate) fn ci_dir(&self, sandbox: &SandboxId) -> PathBuf {
        self.sandbox_dir(sandbox).join("ci")
    }

    /// Whether a run of `sandbox` is executing (in any lane) or its checkouts are claimed.
    pub(crate) fn ci_busy(&self, sandbox: &SandboxId) -> bool {
        self.ci
            .busy
            .lock()
            .map_or(true, |busy| busy_in(&busy, sandbox.as_str()))
    }

    /// Claims `sandbox`'s CI checkouts (the dispatcher starts no run there while claimed), for
    /// work that removes files in them (disk upkeep); `false` when a run executes or another
    /// claim holds them. End it with [`App::ci_release`].
    pub fn ci_claim(&self, sandbox: &SandboxId) -> bool {
        self.ci.busy.lock().is_ok_and(|mut busy| {
            !busy_in(&busy, sandbox.as_str()) && busy.insert(sandbox.as_str().to_owned())
        })
    }

    /// Ends a claim of [`App::ci_claim`] and lets the dispatcher start queued runs.
    pub fn ci_release(&self, sandbox: &SandboxId) {
        if let Ok(mut busy) = self.ci.busy.lock() {
            busy.remove(sandbox.as_str());
        }
        self.ci_wake();
    }

    /// Wakes the dispatcher (a run was queued or a runner freed).
    pub(crate) fn ci_wake(&self) {
        self.ci.wake.notify_one();
    }

    /// The CI jobs `yulius.toml` defines at `rev`: `Ok(empty)` without a file or CI, `Err`
    /// with the problem when the file is invalid.
    pub(crate) async fn ci_jobs_at(&self, repo: &Path, rev: &str) -> Result<Vec<Job>, String> {
        let text = match self.deps.git.show_file(repo, rev, "yulius.toml").await {
            Ok(Some(text)) => text,
            Ok(None) => return Ok(Vec::new()),
            Err(err) => {
                return Err(format!(
                    "yulius.toml can't be read: {}",
                    err.public_message()
                ));
            }
        };
        if text.len() > MAX_CONFIG_BYTES {
            return Err("yulius.toml is larger than 64 KiB".into());
        }
        let config = parse_yulius_toml(&text).map_err(|err| err.to_string())?;
        dom::project_jobs(config.commands.gate.as_deref(), &config.ci).map_err(|e| e.to_string())
    }

    /// The sandbox's CI as configured now (cached for a minute per integration head).
    pub(crate) async fn ci_gate(&self, sandbox: &SandboxRecord) -> GateView {
        let now = self.now().0;
        let repo = self.sandbox_repo(&sandbox.id);
        let profile = delivery_profile(sandbox);
        let base = if repo.join(".git").exists() {
            self.deps
                .git
                .resolve_commit(&repo, &format!("origin/{}", profile.integration_branch))
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        if let Ok(cache) = self.ci.gates.lock()
            && let Some(view) = cache.get(sandbox.id.as_str())
            && view.base_sha == base
            && now - view.read_at < GATE_CACHE_MS
        {
            return view.clone();
        }
        let profile_gate = profile_gate(sandbox);
        let mut view = GateView {
            read_at: now,
            base_sha: base.clone(),
            ..GateView::default()
        };
        let project = match &base {
            Some(sha) => match self.ci_jobs_at(&repo, sha).await {
                Ok(jobs) => jobs,
                Err(problem) => {
                    view.problem = Some(problem);
                    Vec::new()
                }
            },
            None => Vec::new(),
        };
        view.gate = dom::effective_gate(profile_gate.as_deref(), &project);
        if let Some(sha) = &base
            && let Ok(files) = self.deps.git.list_dir(&repo, sha, WORKFLOWS_DIR).await
        {
            view.workflows = files
                .into_iter()
                .filter(|f| f.ends_with(".yml") || f.ends_with(".yaml"))
                .take(MAX_WORKFLOW_FILES)
                .collect();
        }
        if let Ok(mut cache) = self.ci.gates.lock() {
            cache.insert(sandbox.id.as_str().to_owned(), view.clone());
        }
        view
    }

    /// Forgets the cached CI configuration (after a fetch or a profile change).
    pub(crate) fn ci_forget_gate(&self, sandbox: &SandboxId) {
        if let Ok(mut cache) = self.ci.gates.lock() {
            cache.remove(sandbox.as_str());
        }
    }

    /// The CI's verdict at `sha` for the merge gate: the latest run of that commit when there
    /// is one, else pending when the sandbox has CI, else not configured (also when CI is off
    /// on this instance).
    pub async fn ci_gate_result(&self, sandbox: &SandboxRecord, sha: &str) -> GateResult {
        if !self.config.ci.enabled {
            return GateResult::NotConfigured;
        }
        if let Ok(runs) = self.deps.store.ci_runs_for_sha(&sandbox.id, sha).await
            && let Some(latest) = runs.first()
        {
            return GateResult::Run(Some(latest.state));
        }
        if self.ci_gate(sandbox).await.gate.is_some() {
            GateResult::Run(None)
        } else {
            GateResult::NotConfigured
        }
    }

    /// Queues a run of `gate` at `sha`.
    async fn ci_queue(
        &self,
        sandbox: &SandboxRecord,
        reference: CiRef,
        sha: &str,
        gate: &Gate,
        by: &str,
    ) -> AppResult<CiRunRecord> {
        self.ci_queue_placed(sandbox, reference, sha, gate, by, self.now())
            .await
    }

    /// [`App::ci_queue`] with its place in the queue (`queued_at`).
    pub(crate) async fn ci_queue_placed(
        &self,
        sandbox: &SandboxRecord,
        reference: CiRef,
        sha: &str,
        gate: &Gate,
        by: &str,
        queued_at: Timestamp,
    ) -> AppResult<CiRunRecord> {
        let run = CiRunRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: sandbox.id.clone(),
            reference,
            sha: sha.to_owned(),
            source: gate.source,
            state: RunState::Queued,
            reason: None,
            requested_by: by.to_owned(),
            status_for: None,
            status_note: None,
            status_attempts: 0,
            queued_at,
            started_at: None,
            ended_at: None,
        };
        let jobs: Vec<CiJobRecord> = gate
            .jobs
            .iter()
            .enumerate()
            .map(|(index, job)| CiJobRecord {
                id: self.deps.crypto.new_id(),
                run_id: run.id.clone(),
                ordinal: u32::try_from(index).unwrap_or(u32::MAX),
                name: job.name.clone(),
                key: job.key.clone(),
                command: job.command.clone(),
                matrix: job.matrix.clone(),
                state: RunState::Queued,
                reason: None,
                exit_code: None,
                output: String::new(),
                failures: None,
                started_at: None,
                ended_at: None,
                need: job.need,
            })
            .collect();
        self.deps.store.insert_ci_run(&run, &jobs).await?;
        tracing::info!(sandbox = %sandbox.slug, run = %run.id, sha = %short(sha), reference = %run.reference.key(), jobs = jobs.len(), "CI run queued");
        self.ci_publish(&run.id).await;
        self.ci_wake();
        Ok(run)
    }

    /// One scheduling round for a sandbox: fetch, read the heads, queue and supersede.
    ///
    /// # Errors
    /// GitHub or Git failures (the round is retried at the next poll).
    pub async fn ci_sync_sandbox(&self, sandbox: &SandboxRecord) -> AppResult<()> {
        let repo = self.sandbox_repo(&sandbox.id);
        // Yulius CI follows pull requests on GitHub; a playground has neither (SPEC-099).
        if sandbox.removed_at.is_some() || sandbox.is_playground() || !repo.join(".git").exists() {
            return Ok(());
        }
        let token = self.dlv_installation_token(sandbox).await?;
        self.dlv_refresh_clone(sandbox).await?;
        self.ci_forget_gate(&sandbox.id);
        let view = self.ci_gate(sandbox).await;
        let profile = delivery_profile(sandbox);
        let mut targets: Vec<(Target, Gate)> = Vec::new();
        if let Some(gate) = &view.gate {
            let mut branches = vec![profile.integration_branch.clone()];
            if profile.production_branch != profile.integration_branch {
                branches.push(profile.production_branch.clone());
            }
            for branch in branches {
                if let Ok(Some(sha)) = self
                    .deps
                    .git
                    .resolve_commit(&repo, &format!("origin/{branch}"))
                    .await
                {
                    targets.push((
                        Target {
                            reference: CiRef::Branch(branch),
                            sha,
                        },
                        gate.clone(),
                    ));
                }
            }
        }
        let pulls = self
            .deps
            .github
            .list_pulls(&token, &sandbox.repo_owner, &sandbox.repo_name, None)
            .await?;
        for pull in pulls
            .into_iter()
            .filter(|p| p.state == "open" && p.base == profile.integration_branch)
            .take(MAX_PULLS_TESTED)
        {
            // Heads of other repositories (forks) are not in the clone: not tested.
            if !matches!(
                self.deps.git.resolve_commit(&repo, &pull.head_sha).await,
                Ok(Some(_))
            ) {
                continue;
            }
            let own = match self.ci_jobs_at(&repo, &pull.head_sha).await {
                Ok(jobs) if !jobs.is_empty() => Some(jobs),
                _ => None,
            };
            // What the pull request branched off: its own jobs count only when it changed them
            // there, not when the integration branch moved on since (F 2026-10-13: stale branches
            // kept running an apt step `dev` had removed).
            let at_base = match self
                .deps
                .git
                .merge_base(
                    &repo,
                    &pull.head_sha,
                    &format!("origin/{}", profile.integration_branch),
                )
                .await
            {
                Ok(Some(base)) => self.ci_jobs_at(&repo, &base).await.ok(),
                _ => None,
            };
            let gate = match &view.gate {
                // A pull request that changes the CI (an agent scoping a monorepo's jobs to
                // its project, F 2026-10-12) is tested with its own jobs; one that removes them
                // all is still tested with the integration branch's.
                Some(gate) => Some(match own {
                    Some(jobs) if own_jobs_apply(&jobs, at_base.as_deref(), &gate.jobs) => Gate {
                        jobs,
                        source: GateSource::PullRequest,
                    },
                    _ => gate.clone(),
                }),
                // Bootstrap: the integration branch has no CI yet; the pull request that adds
                // it is tested with its own configuration.
                None if view.problem.is_none() => {
                    match self.ci_jobs_at(&repo, &pull.head_sha).await {
                        Ok(jobs) if !jobs.is_empty() => Some(Gate {
                            jobs,
                            source: GateSource::PullRequest,
                        }),
                        _ => None,
                    }
                }
                None => None,
            };
            if let Some(gate) = gate {
                targets.push((
                    Target {
                        reference: CiRef::PullRequest(pull.number),
                        sha: pull.head_sha.clone(),
                    },
                    gate,
                ));
            }
        }
        let runs = self.deps.store.ci_runs(&sandbox.id, PLAN_RUNS).await?;
        // A merge train's run is the train's own (SPEC-084): it starts and cancels it.
        let facts: Vec<RunFacts> = runs
            .iter()
            .filter(|run| !matches!(run.reference, CiRef::Train(_)))
            .map(|run| RunFacts {
                id: run.id.clone(),
                reference: run.reference.clone(),
                sha: run.sha.clone(),
                state: run.state,
            })
            .collect();
        let only_targets: Vec<Target> = targets.iter().map(|(t, _)| t.clone()).collect();
        let plan = dom::plan(&only_targets, &facts);
        // A pull request's new head keeps the place in the queue of the run it supersedes: it
        // went to the back at every push before, behind every other pull request (up to 73
        // minutes of waiting, F 2026-10-12).
        let mut places: HashMap<String, Timestamp> = HashMap::new();
        for (run_id, _) in &plan.supersede {
            if let Some(old) = runs
                .iter()
                .find(|r| &r.id == run_id && r.state == RunState::Queued)
            {
                places.insert(old.reference.key(), old.queued_at);
            }
        }
        for (run_id, successor) in plan.supersede {
            let reason = match successor {
                Some(sha) => format!("superseded by {}", short(&sha)),
                None => "its pull request or branch is gone".to_owned(),
            };
            if let Err(err) = self.ci_cancel(&run_id, &reason).await {
                tracing::warn!(error = %err, run = %run_id, "superseding a CI run failed");
            }
        }
        for target in plan.queue {
            if let Some((_, gate)) = targets.iter().find(|(t, _)| *t == target) {
                // A branch commit whose tree a run already passed (a squash merge of an
                // up-to-date pull request) reuses that result instead of running again: every
                // merge used to cost a second 12-20 minute run in the sandbox's one lane.
                if matches!(target.reference, CiRef::Branch(_))
                    && self.ci_reuse_tree(sandbox, &target, gate, &runs).await?
                {
                    continue;
                }
                let place = places
                    .get(&target.reference.key())
                    .copied()
                    .unwrap_or_else(|| self.now());
                self.ci_queue_placed(
                    sandbox,
                    target.reference.clone(),
                    &target.sha,
                    gate,
                    "yulius",
                    place,
                )
                .await?;
            }
        }
        Ok(())
    }

    /// Records a passed run for `target` when a recent passed run tested a commit with the same
    /// tree (and therefore the same jobs); `false` when there is none.
    async fn ci_reuse_tree(
        &self,
        sandbox: &SandboxRecord,
        target: &Target,
        gate: &Gate,
        runs: &[CiRunRecord],
    ) -> AppResult<bool> {
        let repo = self.sandbox_repo(&sandbox.id);
        let Some(tree) = self.deps.git.tree_of(&repo, &target.sha).await? else {
            return Ok(false);
        };
        let mut source = None;
        // A merge train's verdict for a pull request it merged before the last one covers a tree
        // no run tested (SPEC-084 TRN-005): never a source.
        for run in runs.iter().filter(|r| {
            r.state == RunState::Passed
                && r.sha != target.sha
                && r.requested_by != crate::merge_train::TRAIN_VERDICT_BY
        }) {
            if self.deps.git.tree_of(&repo, &run.sha).await?.as_deref() == Some(tree.as_str()) {
                source = Some(run);
                break;
            }
        }
        let Some(source) = source else {
            return Ok(false);
        };
        let now = self.now();
        let note = format!(
            "not run again: {} has the same tree as {} ({}), which passed",
            short(&target.sha),
            short(&source.sha),
            source.reference.key()
        );
        let run = CiRunRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: sandbox.id.clone(),
            reference: target.reference.clone(),
            sha: target.sha.clone(),
            source: gate.source,
            state: RunState::Passed,
            reason: Some(note.clone()),
            requested_by: "yulius".to_owned(),
            status_for: None,
            status_note: None,
            status_attempts: 0,
            queued_at: now,
            started_at: Some(now),
            ended_at: Some(now),
        };
        let jobs: Vec<CiJobRecord> = gate
            .jobs
            .iter()
            .enumerate()
            .map(|(index, job)| CiJobRecord {
                id: self.deps.crypto.new_id(),
                run_id: run.id.clone(),
                ordinal: u32::try_from(index).unwrap_or(u32::MAX),
                name: job.name.clone(),
                key: job.key.clone(),
                command: job.command.clone(),
                matrix: job.matrix.clone(),
                state: RunState::Passed,
                reason: Some(note.clone()),
                exit_code: Some(0),
                output: format!("{note}.\n"),
                failures: None,
                started_at: Some(now),
                ended_at: Some(now),
                need: job.need,
            })
            .collect();
        self.deps.store.insert_ci_run(&run, &jobs).await?;
        tracing::info!(sandbox = %sandbox.slug, run = %run.id, sha = %short(&target.sha), reused = %short(&source.sha), "CI result reused for the same tree");
        self.ci_publish(&run.id).await;
        Ok(true)
    }

    /// Cancels a run: a queued one at once, a running one through its executor (which kills
    /// its jobs and records the end). `false` when it had already ended.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn ci_cancel(&self, run_id: &str, reason: &str) -> AppResult<bool> {
        let Some(run) = self.deps.store.ci_run(run_id).await? else {
            return Err(AppError::NotFound);
        };
        match run.state {
            RunState::Queued => {
                let update = CiRunUpdate {
                    reason: Some(reason.to_owned()),
                    ended_at: Some(self.now()),
                    ..CiRunUpdate::default()
                };
                if !self
                    .deps
                    .store
                    .transition_ci_run(run_id, &[RunState::Queued], RunState::Cancelled, &update)
                    .await?
                {
                    return Ok(false);
                }
                self.ci_cancel_jobs(run_id, reason).await?;
                self.ci_publish(run_id).await;
                Ok(true)
            }
            RunState::Running => {
                let signalled = self
                    .ci
                    .executing
                    .lock()
                    .ok()
                    .and_then(|map| map.get(run_id).map(|tx| tx.send(Some(reason.to_owned()))))
                    .is_some();
                if signalled {
                    return Ok(true);
                }
                // No executor in this process (it was lost): record the end directly.
                self.ci_cancel_jobs(run_id, reason).await?;
                let update = CiRunUpdate {
                    reason: Some(reason.to_owned()),
                    ended_at: Some(self.now()),
                    ..CiRunUpdate::default()
                };
                let changed = self
                    .deps
                    .store
                    .transition_ci_run(run_id, &[RunState::Running], RunState::Cancelled, &update)
                    .await?;
                self.ci_publish(run_id).await;
                Ok(changed)
            }
            _ => Ok(false),
        }
    }

    async fn ci_cancel_jobs(&self, run_id: &str, reason: &str) -> AppResult<()> {
        for job in self.deps.store.ci_jobs(run_id, false).await? {
            let update = CiJobUpdate {
                reason: Some(reason.to_owned()),
                ended_at: Some(self.now()),
                ..CiJobUpdate::default()
            };
            self.deps
                .store
                .transition_ci_job(
                    &job.id,
                    &[RunState::Queued, RunState::Running],
                    RunState::Cancelled,
                    &update,
                )
                .await?;
        }
        Ok(())
    }

    /// Publishes the commit status of a run's current state (best effort; failures are
    /// recorded on the run and retried by the loop), after a change of the run: the sandbox's
    /// browsers read its pull requests again (SPEC-108 PRL-003).
    pub(crate) async fn ci_publish(&self, run_id: &str) {
        if let Err(err) = self.ci_try_publish(run_id, true).await {
            tracing::debug!(error = %err, run = %run_id, "publishing a CI status failed");
        }
    }

    /// `announce`: the run changed (not a retry of its status), so the browsers are told.
    async fn ci_try_publish(&self, run_id: &str, announce: bool) -> AppResult<()> {
        let store = &self.deps.store;
        let Some(run) = store.ci_run(run_id).await? else {
            return Ok(());
        };
        let Some(sandbox) = store.sandbox(&run.sandbox_id).await? else {
            return Ok(());
        };
        // A merge train's commit exists only in Yulius's clone (SPEC-084): nothing to publish on
        // (the note counts as an attempt, so the retry loop lets it be).
        if matches!(run.reference, CiRef::Train(_)) {
            return store
                .record_ci_status(run_id, None, Some(crate::merge_train::TRAIN_STATUS_NOTE))
                .await;
        }
        // The run's state is what a pull request's checks say (`ci_gate_result`), whether or not
        // GitHub takes the status below.
        if announce {
            self.publish_pulls(&run.sandbox_id);
        }
        let jobs = store.ci_jobs(run_id, false).await?;
        let failed = jobs.iter().find(|j| j.state == RunState::Failed);
        let summary = RunSummary {
            jobs: jobs.len(),
            passed: jobs.iter().filter(|j| j.state == RunState::Passed).count(),
            failed_job: failed.map(|j| (j.name.as_str(), j.exit_code)),
            duration_ms: run
                .started_at
                .zip(run.ended_at)
                .map(|(start, end)| end.millis_since(start)),
            reason: run.reason.as_deref(),
        };
        let status = dom::commit_status(run.state, summary);
        let token = match self.dlv_installation_token(&sandbox).await {
            Ok(token) => token,
            Err(err) => {
                let note = format!("not published: {}", err.public_message());
                store.record_ci_status(run_id, None, Some(&note)).await?;
                return Err(err);
            }
        };
        let request = NewCommitStatus {
            state: status.state.as_str().to_owned(),
            context: dom::STATUS_CONTEXT.to_owned(),
            description: status.description,
            target_url: Some(format!(
                "{}/s/{}/ci?run={}",
                self.config.public_url, sandbox.slug, run.id
            )),
        };
        match self
            .deps
            .github
            .create_commit_status(
                &token,
                &sandbox.repo_owner,
                &sandbox.repo_name,
                &run.sha,
                &request,
            )
            .await
        {
            Ok(()) => store.record_ci_status(run_id, Some(run.state), None).await,
            Err(err) => {
                let note = match &err {
                    AppError::Forbidden(message) => format!("not published: {message}"),
                    other => format!("not published: {}", other.public_message()),
                };
                store.record_ci_status(run_id, None, Some(&note)).await?;
                Err(err)
            }
        }
    }

    /// Starts the CI loops once (no-op when CI is disabled).
    pub fn start_ci(self: &Arc<Self>) {
        if !self.config.ci.enabled || self.ci.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let app = Arc::clone(self);
        tokio::spawn(async move {
            app.ci_recover().await;
            loop {
                app.ci_dispatch().await;
                tokio::select! {
                    () = app.ci.wake.notified() => {}
                    () = tokio::time::sleep(DISPATCH_TICK) => {}
                }
            }
        });
        let app = Arc::clone(self);
        tokio::spawn(async move {
            let mut every = tokio::time::interval(app.config.ci.poll);
            every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                every.tick().await;
                app.ci_sync_all().await;
                app.ci_retry_statuses().await;
                app.ci_prune().await;
            }
        });
        self.start_hygiene();
    }

    /// One sync round over every sandbox.
    pub async fn ci_sync_all(&self) {
        let Ok(sandboxes) = self.deps.store.sandboxes().await else {
            return;
        };
        for sandbox in sandboxes.iter().filter(|s| s.removed_at.is_none()) {
            if let Err(err) = self.ci_sync_sandbox(sandbox).await {
                tracing::debug!(sandbox = %sandbox.slug, error = %err, "CI sync failed");
            }
        }
    }

    async fn ci_retry_statuses(&self) {
        let since = self.now().plus_millis(-STATUS_RETRY_WINDOW_MS);
        let Ok(runs) = self
            .deps
            .store
            .unpublished_ci_runs(since, STATUS_MAX_ATTEMPTS, 50)
            .await
        else {
            return;
        };
        for run in runs {
            // A retry changes nothing a pull request's checks say: no announcement.
            if let Err(err) = self.ci_try_publish(&run.id, false).await {
                tracing::debug!(error = %err, run = %run.id, "publishing a CI status failed");
            }
        }
    }

    async fn ci_prune(&self) {
        let now = self.now().0;
        {
            let Ok(mut last) = self.ci.last_prune.lock() else {
                return;
            };
            if now - *last < PRUNE_EVERY_MS {
                return;
            }
            *last = now;
        }
        match self
            .deps
            .store
            .prune_ci_runs(KEEP_RUNS, Timestamp(now - RETAIN_MS))
            .await
        {
            Ok(0) => {}
            Ok(n) => tracing::info!(runs = n, "old CI runs deleted"),
            Err(err) => tracing::warn!(error = %err, "pruning CI runs failed"),
        }
        self.cd_prune().await;
        self.prune_spans(Timestamp(now)).await;
    }

    /// Starts the oldest queued runs of every sandbox, as many as it has free lanes
    /// ([`CiSettings::lanes`]); the instance-wide job slots still bound the machine's load.
    pub async fn ci_dispatch(self: &Arc<Self>) {
        let Ok(active) = self.deps.store.active_ci_runs().await else {
            return;
        };
        let lanes = self.config.ci.lanes.max(1);
        for run in active.into_iter().filter(|r| r.state == RunState::Queued) {
            // Over the disk quota a run waits in the queue instead of erroring (CI-301); asked
            // only when a lane is free.
            let lane_free = self
                .ci
                .busy
                .lock()
                .is_ok_and(|busy| free_lane(&busy, run.sandbox_id.as_str(), lanes).is_some());
            if !lane_free || !self.ci_disk_room(&run.sandbox_id).await {
                continue;
            }
            let sandbox = run.sandbox_id.as_str().to_owned();
            let (tx, rx) = watch::channel(None);
            let lane = {
                let Ok(mut busy) = self.ci.busy.lock() else {
                    return;
                };
                let Some(lane) = take_lane(&mut busy, &sandbox, lanes) else {
                    continue;
                };
                if let Ok(mut executing) = self.ci.executing.lock() {
                    executing.insert(run.id.clone(), tx);
                }
                lane
            };
            let app = Arc::clone(self);
            tokio::spawn(async move { app.ci_execute(run, rx, lane).await });
        }
    }

    /// After a restart: runs recorded as running have no executor; their hosts are ended and
    /// they are queued again.
    async fn ci_recover(&self) {
        let Ok(active) = self.deps.store.active_ci_runs().await else {
            return;
        };
        for run in active.into_iter().filter(|r| r.state == RunState::Running) {
            if self
                .ci
                .executing
                .lock()
                .is_ok_and(|map| map.contains_key(&run.id))
            {
                continue;
            }
            if let Ok(jobs) = self.deps.store.ci_jobs(&run.id, false).await {
                for job in jobs {
                    let dir = self.ci_run_dir(&job.id);
                    if dir.exists() {
                        let _ = self.deps.session_host.terminate(&dir);
                        let _ = std::fs::remove_dir_all(&dir);
                    }
                }
            }
            match self
                .deps
                .store
                .requeue_ci_run(
                    &run.id,
                    "[yulius] queued again after a restart of Yulius\n",
                    self.now(),
                )
                .await
            {
                Ok(true) => tracing::info!(run = %run.id, "CI run queued again after a restart"),
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(run = %run.id, error = %err, "requeueing a CI run failed")
                }
            }
        }
    }

    fn ci_run_dir(&self, job_id: &str) -> PathBuf {
        self.config
            .data_dir
            .join("runs")
            .join(format!("ci-{job_id}"))
    }

    // ---------------------------------------------------------------- API

    async fn ci_sandbox(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        action: Action,
    ) -> AppResult<(SandboxRecord, AccessLevel)> {
        let sid = SandboxId::parse(sandbox_id).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, action).await
    }

    async fn ci_run_of(&self, sandbox: &SandboxRecord, run_id: &str) -> AppResult<CiRunRecord> {
        self.deps
            .store
            .ci_run(run_id)
            .await?
            .filter(|run| run.sandbox_id == sandbox.id)
            .ok_or(AppError::NotFound)
    }

    /// `GET /api/sandboxes/{sid}/ci`: the CI in force, recent runs (without outputs) and, for
    /// administrators, whether statuses can be published.
    ///
    /// # Errors
    /// Access errors, store failures.
    pub async fn ci_overview(&self, ctx: &AuthContext, sandbox_id: &str) -> AppResult<CiOverview> {
        let (sandbox, level) = self.ci_sandbox(ctx, sandbox_id, Action::View).await?;
        let view = self.ci_gate(&sandbox).await;
        let records = self.deps.store.ci_runs(&sandbox.id, RUN_LIST_LIMIT).await?;
        let mut runs = Vec::with_capacity(records.len());
        for run in &records {
            let jobs = self.deps.store.ci_jobs(&run.id, false).await?;
            runs.push(run_view(run, &jobs, false));
        }
        let admin = level >= AccessLevel::Admin || ctx.user.is_superadmin;
        let status_access = records
            .iter()
            .find(|run| run.status_for.is_some() || run.status_note.is_some())
            .and_then(|run| run.status_note.as_deref())
            .filter(|note| admin && note.contains(STATUS_PERMISSION_MISSING))
            .map(|_| {
                format!(
                    "Yulius can't publish its CI results on {}/{}: {STATUS_PERMISSION_MISSING}. The runs and the merge gate work without it.",
                    sandbox.repo_owner, sandbox.repo_name
                )
            });
        Ok(CiOverview {
            gate: view.gate.as_ref().map(gate_view),
            gate_problem: view.problem,
            runs,
            can_run: level >= AccessLevel::Write,
            status_access,
            actions_workflows: view.workflows,
        })
    }

    /// `GET /api/sandboxes/{sid}/ci/runs/{rid}`: one run with its jobs' output.
    ///
    /// # Errors
    /// Access errors, `NotFound`.
    pub async fn ci_run_detail(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        run_id: &str,
    ) -> AppResult<CiRun> {
        let (sandbox, _) = self.ci_sandbox(ctx, sandbox_id, Action::View).await?;
        let run = self.ci_run_of(&sandbox, run_id).await?;
        let jobs = self.deps.store.ci_jobs(&run.id, true).await?;
        Ok(run_view(&run, &jobs, true))
    }

    /// `POST /api/sandboxes/{sid}/ci/runs/{rid}/cancel`.
    ///
    /// # Errors
    /// Access errors, `NotFound`, `Conflict` when it already ended.
    pub async fn ci_cancel_run(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        run_id: &str,
    ) -> AppResult<CiRun> {
        let (sandbox, _) = self.ci_sandbox(ctx, sandbox_id, Action::Work).await?;
        let run = self.ci_run_of(&sandbox, run_id).await?;
        let actor = Self::dlv_actor_login(&ctx.user);
        if !self
            .ci_cancel(&run.id, &format!("cancelled by {actor}"))
            .await?
        {
            return Err(AppError::Conflict("this run has already ended".into()));
        }
        self.dlv_audit(
            &actor,
            "ci.cancel",
            &sandbox.slug,
            &format!("run {} at {}", run.id, run.sha),
        )
        .await;
        let run = self.ci_run_of(&sandbox, run_id).await?;
        let jobs = self.deps.store.ci_jobs(&run.id, false).await?;
        Ok(run_view(&run, &jobs, false))
    }

    /// Queues the commit of `run_id` again with the CI in force now.
    ///
    /// # Errors
    /// `Conflict` while a run of that commit is active or without CI.
    pub(crate) async fn ci_rerun(
        &self,
        sandbox: &SandboxRecord,
        run_id: &str,
        by: &str,
    ) -> AppResult<CiRunRecord> {
        let run = self.ci_run_of(sandbox, run_id).await?;
        if matches!(run.reference, CiRef::Train(_)) {
            return Err(AppError::Conflict(
                "a merge train is not run again: Yulius builds a new one from the armed merges"
                    .into(),
            ));
        }
        let others = self
            .deps
            .store
            .ci_runs_for_sha(&sandbox.id, &run.sha)
            .await?;
        if others.iter().any(|r| r.state.is_active()) {
            return Err(AppError::Conflict(
                "a run of this commit is already queued or running".into(),
            ));
        }
        self.ci_forget_gate(&sandbox.id);
        let view = self.ci_gate(sandbox).await;
        let gate = match view.gate {
            Some(gate) => gate,
            None if run.source == GateSource::PullRequest => {
                let repo = self.sandbox_repo(&sandbox.id);
                let jobs = self
                    .ci_jobs_at(&repo, &run.sha)
                    .await
                    .map_err(AppError::Conflict)?;
                if jobs.is_empty() {
                    return Err(AppError::Conflict(
                        "this commit has no CI configured".into(),
                    ));
                }
                Gate {
                    jobs,
                    source: GateSource::PullRequest,
                }
            }
            None => {
                return Err(AppError::Conflict(
                    view.problem
                        .unwrap_or_else(|| "this sandbox has no CI configured".into()),
                ));
            }
        };
        self.ci_queue(sandbox, run.reference.clone(), &run.sha, &gate, by)
            .await
    }

    /// `POST /api/sandboxes/{sid}/ci/runs/{rid}/rerun`.
    ///
    /// # Errors
    /// Access errors, `NotFound`, `Conflict`.
    pub async fn ci_rerun_run(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        run_id: &str,
    ) -> AppResult<CiRun> {
        let (sandbox, _) = self.ci_sandbox(ctx, sandbox_id, Action::Work).await?;
        let actor = Self::dlv_actor_login(&ctx.user);
        let run = self.ci_rerun(&sandbox, run_id, &actor).await?;
        self.dlv_audit(
            &actor,
            "ci.rerun",
            &sandbox.slug,
            &format!("{} at {}", run.reference.key(), run.sha),
        )
        .await;
        let jobs = self.deps.store.ci_jobs(&run.id, false).await?;
        Ok(run_view(&run, &jobs, false))
    }

    /// `POST /api/sandboxes/{sid}/ci/sync`: looks for new heads now.
    ///
    /// # Errors
    /// Access errors, GitHub or Git failures.
    pub async fn ci_sync_now(&self, ctx: &AuthContext, sandbox_id: &str) -> AppResult<CiOverview> {
        let (sandbox, _) = self.ci_sandbox(ctx, sandbox_id, Action::Work).await?;
        self.ci_sync_sandbox(&sandbox).await?;
        self.ci_overview(ctx, sandbox_id).await
    }
}

#[cfg(test)]
mod lane_tests {
    use super::{busy_in, own_jobs_apply, take_lane};

    /// CI-110: a stale branch is not tested with the integration branch's old jobs it still
    /// carries; a pull request that changed the jobs is tested with its own.
    #[test]
    fn a_pull_request_runs_its_own_jobs_only_when_it_changed_them() {
        let (old, new, scoped) = (["apt"], ["no-apt"], ["no-apt", "scoped"]);
        assert!(
            !own_jobs_apply(&old, Some(&old[..]), &new),
            "behind: dev's jobs"
        );
        assert!(
            own_jobs_apply(&scoped, Some(&new[..]), &new),
            "changed by the pull request"
        );
        assert!(!own_jobs_apply(&new, Some(&old[..]), &new), "same as dev's");
        assert!(own_jobs_apply(&old, None, &new), "base unknown: as before");
    }
    use std::collections::HashSet;

    #[test]
    fn a_sandbox_runs_as_many_runs_as_it_has_lanes_and_never_while_claimed() {
        let mut busy = HashSet::new();
        assert_eq!(take_lane(&mut busy, "s1", 2), Some(0));
        assert_eq!(take_lane(&mut busy, "s1", 2), Some(1));
        assert_eq!(take_lane(&mut busy, "s1", 2), None, "both lanes run");
        assert_eq!(
            take_lane(&mut busy, "s2", 2),
            Some(0),
            "sandboxes are independent"
        );
        assert!(
            busy_in(&busy, "s1") && !busy_in(&busy, "s"),
            "no prefix confusion"
        );
        busy.remove("s1/0");
        assert_eq!(
            take_lane(&mut busy, "s1", 2),
            Some(0),
            "a freed lane is reused"
        );
        let mut claimed: HashSet<String> = ["s3".to_owned()].into();
        assert_eq!(
            take_lane(&mut claimed, "s3", 2),
            None,
            "a disk claim holds every lane"
        );
    }
}
