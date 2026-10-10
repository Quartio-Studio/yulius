//! Yulius-run CI (SPEC-001 DLV-006, SPEC-002 ADR-18) and repository hygiene (DLV-007).
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
//!   (one run per sandbox at a time: its jobs use the sandbox's persistent CI checkouts);
//! - a run's jobs run in parallel, each holding one of [`CiSettings::max_jobs`] instance-wide
//!   slots; a job runs through the process host ([`crate::ports::session::SessionHost`]) inside
//!   the sandbox's containment, with no secret in its environment, for at most
//!   [`CiSettings::job_timeout`]; cancellation and timeouts kill the job's process group, and
//!   the host is terminated when the exit is not seen within [`KILL_GRACE`];
//! - output is the job's log file, read while it runs, kept as the last [`OUTPUT_BYTES`]
//!   (escape sequences removed); a log beyond [`MAX_LOG_FILE_BYTES`] ends the job;
//! - retention: [`KEEP_RUNS`] runs per sandbox and [`RETAIN_MS`]; commit statuses are retried
//!   [`STATUS_MAX_ATTEMPTS`] times within a day.
//!
//! Restart: runs recorded as running lose their executor; their hosts are terminated and they
//! go back to the queue (a run is repeatable).

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
    self as dom, CiRef, Gate, GateResult, GateSource, Job, RunFacts, RunState, RunSummary,
    Target,
};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::{AccessLevel, Action};
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
    /// Started at.
    pub started_at: Option<Timestamp>,
    /// Ended at.
    pub ended_at: Option<Timestamp>,
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
    /// Executing runs: their cancellation (the reason, once cancelled).
    executing: Mutex<HashMap<String, watch::Sender<Option<String>>>>,
    /// Sandboxes with an executing run.
    busy: Mutex<HashSet<String>>,
    gates: Mutex<HashMap<String, GateView>>,
    pub(crate) hygiene: Mutex<HashMap<String, yulius_contracts::api::ci::HygieneReport>>,
    wake: Notify,
    started: AtomicBool,
    last_prune: Mutex<i64>,
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
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
            CiRef::Branch(_) => None,
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

    /// Wakes the dispatcher (a run was queued or a runner freed).
    pub(crate) fn ci_wake(&self) {
        self.ci.wake.notify_one();
    }

    /// The CI jobs `yulius.toml` defines at `rev`: `Ok(empty)` without a file or CI, `Err`
    /// with the problem when the file is invalid.
    async fn ci_jobs_at(&self, repo: &Path, rev: &str) -> Result<Vec<Job>, String> {
        let text = match self.deps.git.show_file(repo, rev, "yulius.toml").await {
            Ok(Some(text)) => text,
            Ok(None) => return Ok(Vec::new()),
            Err(err) => return Err(format!("yulius.toml can't be read: {}", err.public_message())),
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
            queued_at: self.now(),
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
                started_at: None,
                ended_at: None,
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
        if sandbox.removed_at.is_some() || !repo.join(".git").exists() {
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
            if !matches!(self.deps.git.resolve_commit(&repo, &pull.head_sha).await, Ok(Some(_))) {
                continue;
            }
            let gate = match &view.gate {
                Some(gate) => Some(gate.clone()),
                // Bootstrap: the integration branch has no CI yet; the pull request that adds
                // it is tested with its own configuration.
                None if view.problem.is_none() => match self.ci_jobs_at(&repo, &pull.head_sha).await
                {
                    Ok(jobs) if !jobs.is_empty() => Some(Gate {
                        jobs,
                        source: GateSource::PullRequest,
                    }),
                    _ => None,
                },
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
        let facts: Vec<RunFacts> = runs
            .iter()
            .map(|run| RunFacts {
                id: run.id.clone(),
                reference: run.reference.clone(),
                sha: run.sha.clone(),
                state: run.state,
            })
            .collect();
        let only_targets: Vec<Target> = targets.iter().map(|(t, _)| t.clone()).collect();
        let plan = dom::plan(&only_targets, &facts);
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
                self.ci_queue(sandbox, target.reference.clone(), &target.sha, gate, "yulius")
                    .await?;
            }
        }
        Ok(())
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
    /// recorded on the run and retried by the loop).
    pub(crate) async fn ci_publish(&self, run_id: &str) {
        if let Err(err) = self.ci_try_publish(run_id).await {
            tracing::debug!(error = %err, run = %run_id, "publishing a CI status failed");
        }
    }

    async fn ci_try_publish(&self, run_id: &str) -> AppResult<()> {
        let store = &self.deps.store;
        let Some(run) = store.ci_run(run_id).await? else {
            return Ok(());
        };
        let Some(sandbox) = store.sandbox(&run.sandbox_id).await? else {
            return Ok(());
        };
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
            self.ci_publish(&run.id).await;
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
    }

    /// Starts every queued run whose sandbox has no executing run.
    pub async fn ci_dispatch(self: &Arc<Self>) {
        let Ok(active) = self.deps.store.active_ci_runs().await else {
            return;
        };
        let running: HashSet<String> = active
            .iter()
            .filter(|run| run.state == RunState::Running)
            .map(|run| run.sandbox_id.as_str().to_owned())
            .collect();
        for run in active.into_iter().filter(|r| r.state == RunState::Queued) {
            let sandbox = run.sandbox_id.as_str().to_owned();
            if running.contains(&sandbox) {
                continue;
            }
            let (tx, rx) = watch::channel(None);
            {
                let Ok(mut busy) = self.ci.busy.lock() else {
                    return;
                };
                if !busy.insert(sandbox.clone()) {
                    continue;
                }
                if let Ok(mut executing) = self.ci.executing.lock() {
                    executing.insert(run.id.clone(), tx);
                }
            }
            let app = Arc::clone(self);
            tokio::spawn(async move { app.ci_execute(run, rx).await });
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
            match self.deps.store.requeue_ci_run(&run.id).await {
                Ok(true) => tracing::info!(run = %run.id, "CI run queued again after a restart"),
                Ok(false) => {}
                Err(err) => tracing::warn!(run = %run.id, error = %err, "requeueing a CI run failed"),
            }
        }
    }

    fn ci_run_dir(&self, job_id: &str) -> PathBuf {
        self.config.data_dir.join("runs").join(format!("ci-{job_id}"))
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
        self.dlv_audit(&actor, "ci.cancel", &sandbox.slug, &format!("run {} at {}", run.id, run.sha))
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
        let others = self.deps.store.ci_runs_for_sha(&sandbox.id, &run.sha).await?;
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
                    return Err(AppError::Conflict("this commit has no CI configured".into()));
                }
                Gate {
                    jobs,
                    source: GateSource::PullRequest,
                }
            }
            None => {
                return Err(AppError::Conflict(view.problem.unwrap_or_else(|| {
                    "this sandbox has no CI configured".into()
                })));
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
        self.dlv_audit(&actor, "ci.rerun", &sandbox.slug, &format!("{} at {}", run.reference.key(), run.sha))
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
