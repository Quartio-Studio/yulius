//! Executing a CI run: its jobs in parallel through the process host, inside the sandbox's
//! containment (SPEC-002 ADR-21; RUN-001..003).
//!
//! A job is `/bin/sh -c <wrapper> yulius-ci <log> <command>`: the fixed wrapper sends the
//! job's output to its log file and then runs the command as its own `/bin/sh -c` argument, so
//! the configured shell text is never spliced into Yulius's script. The log is read while the
//! job runs (complete from the first byte, unlike a terminal attached after the start) and its
//! tail stored. The environment has no secret: no task token, provider credential or GitHub
//! token. A job the integration branch's `[[ci.jobs]]` entry grants agent secrets gets their
//! placeholders and the egress proxy with a credential of its own, never a value (SPEC-142,
//! [`App::ci_job_secrets`]). `HOME` and `YULIUS_CI_CACHE` are persistent per sandbox and shared
//! by its lanes (up to [`super::CiSettings::lanes`] runs at once, so a job's installs there must
//! tolerate a concurrent run), and each job keeps a persistent checkout per lane
//! ([`checkout_name`]), so caches (dependency downloads, build outputs) survive between runs.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::watch;
use yulius_domain::ci::failures::FailureScanner;
use yulius_domain::ci::{self as dom, Gate, GateSource, RunState};
use yulius_domain::resources::{Budget, budget_env};
use yulius_domain::vault::{SecretName, placeholder};

use super::{
    CiJobRecord, CiJobUpdate, CiRunRecord, CiRunUpdate, KILL_GRACE, MAX_LOG_FILE_BYTES,
    OUTPUT_BYTES,
};
use crate::context::App;
use crate::delivery::runner::OutputRing;
use crate::error::AppError;
use crate::model::SandboxRecord;
use crate::ports::isolation::SandboxJoin;
use crate::ports::session::{Containment, LaunchPlan, RunEvent, RunExit, RunInspection, RunSignal};
use crate::resources::repository_templates;
use crate::stages::{NewSpan, OutputStages};
use yulius_domain::stages::SubjectKind;

/// The fixed wrapper: output to the log, stdin closed, then the configured command.
const WRAPPER: &str = "exec >>\"$1\" 2>&1 </dev/null; exec /bin/sh -c \"$2\"";
/// How often the log is read and the stored output refreshed.
const READ_EVERY: Duration = Duration::from_millis(500);
const FLUSH_EVERY_TICKS: u32 = 4;
/// Bytes read from the log per tick (the rest follows on the next ticks).
const READ_CHUNK: usize = 1 << 20;

/// Removes terminal escape sequences (CSI, OSC and two-byte escapes) and turns carriage
/// returns into line breaks, across chunk boundaries.
#[derive(Debug, Default)]
pub(super) struct AnsiStripper {
    state: Strip,
    last_cr: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Strip {
    #[default]
    Text,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

impl AnsiStripper {
    pub(super) fn push(&mut self, input: &[u8], out: &mut Vec<u8>) {
        for &byte in input {
            match self.state {
                Strip::Text => match byte {
                    0x1b => self.state = Strip::Escape,
                    b'\r' => {
                        out.push(b'\n');
                        self.last_cr = true;
                        continue;
                    }
                    b'\n' if self.last_cr => {}
                    0x08 | 0x07 => {}
                    _ => out.push(byte),
                },
                Strip::Escape => {
                    self.state = match byte {
                        b'[' => Strip::Csi,
                        b']' => Strip::Osc,
                        _ => Strip::Text,
                    }
                }
                Strip::Csi => {
                    if (0x40..=0x7e).contains(&byte) {
                        self.state = Strip::Text;
                    }
                }
                Strip::Osc => match byte {
                    0x07 => self.state = Strip::Text,
                    0x1b => self.state = Strip::OscEscape,
                    _ => {}
                },
                Strip::OscEscape => self.state = Strip::Text,
            }
            self.last_cr = false;
        }
    }
}

/// A CI job joins the sandbox's CI group, weighted above the agents' (SPEC-053 CPU-203), so its
/// speed does not depend on what the agents build.
pub(super) fn ci_join(join: SandboxJoin) -> SandboxJoin {
    SandboxJoin {
        cgroup: join.ci_cgroup.clone().or(join.cgroup),
        ..join
    }
}

/// Where a watched process's output goes while it runs: a CI job's row, or a CD run's
/// (SPEC-134), redacted of the sandbox's agent secrets first.
pub(super) enum Sink<'a> {
    /// A CI job.
    CiJob(&'a str),
    /// A CD run of a sandbox.
    CdRun(&'a str, &'a yulius_domain::ids::SandboxId),
}

/// What a watched process used, when its host measured it (SPEC-154). Unknown for a job Yulius
/// killed (cancelled, timed out, over its output): the group kill sends its processes to init,
/// out of the host's reaping chain, so what the host measured would read as almost nothing.
pub(super) type Used = Option<yulius_domain::stages::ResourceUse>;

/// How a job's process ended.
pub(super) enum Ended {
    Exit(RunExit),
    Cancelled(String),
    TimedOut,
    OutputLimit,
    Lost,
    /// The sandbox refused the start over its disk quota (SPEC-059 CI-303).
    NoDisk(String),
}

/// Whether a run whose jobs ended in `states` goes back to the queue to wait for disk (CI-303):
/// `no_disk` of its jobs were refused over the quota, every error is one of those, and nothing
/// failed. Jobs those refusals stopped end cancelled; a real error or failure keeps its verdict.
fn requeue_for_disk(states: &[RunState], no_disk: usize) -> bool {
    no_disk > 0
        && !states.contains(&RunState::Failed)
        && states.iter().filter(|s| **s == RunState::Error).count() == no_disk
}

/// Why a run a job refusal queued again waits, until the next check says more.
const NO_DISK_REASON: &str = "waiting for disk space: a job was refused over the disk quota; \
                              starts by itself once usage is under the quota";

/// What a job refused over the disk quota leaves in its output when its run is queued again.
const NO_DISK_REQUEUE: &str =
    "[yulius] queued again: the sandbox was over its disk quota when this run started\n";

/// The agent secrets a job of definition `key` may use (SPEC-142 CSE-002): what the integration
/// branch's entry of that name grants. Only the integration branch's `[[ci.jobs]]` grant; a
/// profile gate grants nothing, and neither does a pull request's own configuration (it never
/// reaches `gate`, which [`App::ci_gate`] reads from the integration branch).
fn granted(gate: Option<&Gate>, key: &str) -> Vec<String> {
    gate.filter(|gate| gate.source == GateSource::ProjectConfig)
        .and_then(|gate| gate.jobs.iter().find(|job| job.key == key))
        .map(|job| job.secrets.clone())
        .unwrap_or_default()
}

/// A CI job's proxy credential (SPEC-142 CSE-004): closed when dropped, so it ends on every
/// path out of the job, a cancelled or panicking one included.
struct CiProxy<'a> {
    app: &'a App,
    token: String,
}

impl Drop for CiProxy<'_> {
    fn drop(&mut self) {
        self.app.ci_egress_close(&self.token);
    }
}

/// Binds the files a plan writes outside the sandbox (the CA bundle in the run directory)
/// read-only into its containment, at the same path, as agent runs do.
fn bind_run_files(plan: &mut LaunchPlan) {
    if let Containment::Bubblewrap {
        readonly,
        sandbox_root,
        ..
    } = &mut plan.containment
    {
        for file in &plan.files {
            if !file.path.starts_with(&*sandbox_root) && !readonly.contains(&file.path) {
                readonly.push(file.path.clone());
            }
        }
    }
}

/// A CI job's PATH: the server's, with the directory of the `yulius` binary first when it is not
/// on it, so `yulius security-scan` runs in every job (SPEC-195 VSC-001).
fn ci_path(path: &str, exe: &Path) -> String {
    let Some(dir) = exe.parent().filter(|d| d.is_absolute()) else {
        return path.to_owned();
    };
    let dir = dir.to_string_lossy();
    if path.split(':').any(|entry| entry == dir) {
        path.to_owned()
    } else if path.is_empty() {
        dir.into_owned()
    } else {
        format!("{dir}:{path}")
    }
}

/// The directory of a job's persistent checkout: the definition's name, plus the matrix value's
/// position for matrix jobs (siblings run in parallel and need their own), plus the lane past
/// the first (two runs of a sandbox run in parallel and need their own too).
fn checkout_name(job: &CiJobRecord, siblings: &[CiJobRecord], lane: u32) -> String {
    let same: Vec<&CiJobRecord> = siblings.iter().filter(|j| j.key == job.key).collect();
    let name = if same.len() <= 1 {
        job.key.clone()
    } else {
        let index = same.iter().position(|j| j.id == job.id).unwrap_or(0) + 1;
        format!("{}.{index}", job.key)
    };
    if lane == 0 {
        name
    } else {
        format!("{name}@{lane}")
    }
}

impl App {
    /// Executes one run in `lane` (the dispatcher marked it busy); always releases it.
    pub(super) async fn ci_execute(
        self: Arc<Self>,
        run: CiRunRecord,
        cancel: watch::Receiver<Option<String>>,
        lane: u32,
    ) {
        let own = cancel.clone();
        self.ci_execute_inner(&run, cancel, lane).await;
        self.ci_forget_executor(&run.id, &own);
        if let Ok(mut busy) = self.ci.busy.lock() {
            busy.remove(&format!("{}/{lane}", run.sandbox_id.as_str()));
        }
        self.ci_wake();
    }

    /// Forgets `run_id`'s executor when it is still the one listening on `own`: a run queued
    /// again for disk may already run in another lane, with its own.
    fn ci_forget_executor(&self, run_id: &str, own: &watch::Receiver<Option<String>>) {
        if let Ok(mut executing) = self.ci.executing.lock()
            && executing
                .get(run_id)
                .is_some_and(|tx| own.same_channel(&tx.subscribe()))
        {
            executing.remove(run_id);
        }
    }

    async fn ci_execute_inner(
        self: &Arc<Self>,
        run: &CiRunRecord,
        cancel: watch::Receiver<Option<String>>,
        lane: u32,
    ) {
        let store = &self.deps.store;
        let start = CiRunUpdate {
            started_at: Some(self.now()),
            ..CiRunUpdate::default()
        };
        match store
            .transition_ci_run(&run.id, &[RunState::Queued], RunState::Running, &start)
            .await
        {
            Ok(true) => {}
            Ok(false) => return,
            Err(err) => {
                tracing::warn!(run = %run.id, error = %err, "starting a CI run failed");
                return;
            }
        }
        self.ci_publish(&run.id).await;
        let jobs = store.ci_jobs(&run.id, false).await.unwrap_or_default();
        let sandbox = match store.sandbox(&run.sandbox_id).await {
            Ok(Some(sandbox)) if sandbox.removed_at.is_none() => Some(sandbox),
            _ => None,
        };
        let preparing = self.now();
        let prepared = match &sandbox {
            Some(sandbox) => self.ci_prepare(sandbox, run).await,
            None => Err("the sandbox was removed".to_owned()),
        };
        self.record_span(NewSpan {
            sandbox_id: run.sandbox_id.clone(),
            subject_kind: SubjectKind::CiRun,
            subject_id: run.id.clone(),
            key: format!("prepare@{}", preparing.0),
            parent_key: None,
            kind: "prepare".to_owned(),
            name: "commit".to_owned(),
            environment: None,
            job: None,
            started_at: preparing,
            ended_at: Some(self.now()),
            outcome: Some(if prepared.is_ok() { "passed" } else { "error" }.to_owned()),
            detail: None,
        })
        .await;
        let mut no_disk = 0usize;
        match (sandbox, prepared) {
            (Some(sandbox), Ok(())) => {
                let sandbox = Arc::new(sandbox);
                // Fail fast: the run's jobs share a cancel signal that follows the run's own
                // (superseded, stopped) and fires when one of them fails, so a failed `quality`
                // no longer leaves `e2e` holding the sandbox's CI lane for half an hour.
                let (siblings_tx, siblings) = watch::channel(cancel.borrow().clone());
                let siblings_tx = Arc::new(siblings_tx);
                {
                    let mut outer = cancel.clone();
                    let forward = Arc::clone(&siblings_tx);
                    tokio::spawn(async move {
                        while outer.changed().await.is_ok() {
                            let reason = outer.borrow().clone();
                            if reason.is_some() {
                                let _ = forward.send(reason);
                                break;
                            }
                        }
                    });
                }
                let mut handles = Vec::with_capacity(jobs.len());
                let share = jobs.len();
                for job in &jobs {
                    let app = Arc::clone(self);
                    let (sandbox, run, job) = (Arc::clone(&sandbox), run.clone(), job.clone());
                    let checkout = checkout_name(&job, &jobs, lane);
                    let job_cancel = siblings.clone();
                    let stop_siblings = Arc::clone(&siblings_tx);
                    handles.push(tokio::spawn(async move {
                        let no_disk = Arc::clone(&app)
                            .ci_job(&sandbox, &run, &job, share, &checkout, job_cancel)
                            .await;
                        let failed =
                            app.deps
                                .store
                                .ci_jobs(&run.id, false)
                                .await
                                .is_ok_and(|all| {
                                    all.iter()
                                        .any(|j| j.id == job.id && j.state == RunState::Failed)
                                });
                        if (failed || no_disk) && stop_siblings.borrow().is_none() {
                            let reason = if no_disk {
                                "stopped early: the sandbox is over its disk quota".to_owned()
                            } else {
                                format!("stopped early: job {} failed", job.name)
                            };
                            let _ = stop_siblings.send(Some(reason));
                        }
                        no_disk
                    }));
                }
                for handle in handles {
                    no_disk += usize::from(handle.await.unwrap_or(false));
                }
            }
            (_, prepared) => {
                let reason = prepared
                    .err()
                    .unwrap_or_else(|| "the sandbox was removed".to_owned());
                for job in &jobs {
                    let _ = store
                        .transition_ci_job(
                            &job.id,
                            &[RunState::Queued],
                            RunState::Error,
                            &CiJobUpdate {
                                reason: Some(reason.clone()),
                                ended_at: Some(self.now()),
                                ..CiJobUpdate::default()
                            },
                        )
                        .await;
                }
            }
        }
        let jobs = store.ci_jobs(&run.id, false).await.unwrap_or_default();
        let states: Vec<RunState> = jobs.iter().map(|j| j.state).collect();
        let mut state = dom::aggregate(&states);
        if state.is_active() {
            // Every job task ended: an active job here lost its runner.
            state = RunState::Error;
        }
        if cancel.borrow().is_some() {
            // A cancelled run (superseded or stopped) ends cancelled whatever its killed jobs
            // reported: a job killed while it was starting can end as an error (a slow host
            // loses that race), and the run must not then read as broken.
            state = RunState::Cancelled;
        }
        // A job the sandbox refused over its quota (usage crossed it after the dispatcher's
        // check) and nothing failed: the run waits for disk in the queue (CI-303).
        if cancel.borrow().is_none() && requeue_for_disk(&states, no_disk) {
            match store
                .requeue_ci_run(&run.id, NO_DISK_REQUEUE, self.now())
                .await
            {
                Ok(true) => {
                    tracing::info!(run = %run.id, "CI run queued again: the sandbox is over its disk quota");
                    self.ci_disk_backoff(&run.sandbox_id);
                    // From here a cancel finds the run queued; one that signalled the executor
                    // meanwhile still ends it.
                    self.ci_forget_executor(&run.id, &cancel);
                    let late = cancel.borrow().clone();
                    match late {
                        Some(reason) => {
                            let _ = self.ci_cancel(&run.id, &reason).await;
                        }
                        None => {
                            let note = CiRunUpdate {
                                reason: Some(NO_DISK_REASON.to_owned()),
                                ..CiRunUpdate::default()
                            };
                            let _ = store
                                .transition_ci_run(
                                    &run.id,
                                    &[RunState::Queued],
                                    RunState::Queued,
                                    &note,
                                )
                                .await;
                            self.ci_publish(&run.id).await;
                        }
                    }
                    return;
                }
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(run = %run.id, error = %err, "requeueing a CI run failed");
                }
            }
        }
        let reason = match state {
            RunState::Cancelled => cancel
                .borrow()
                .clone()
                .or_else(|| first_reason(&jobs, state)),
            RunState::Error => first_reason(&jobs, state),
            _ => None,
        };
        let end = CiRunUpdate {
            reason,
            ended_at: Some(self.now()),
            ..CiRunUpdate::default()
        };
        if let Err(err) = store
            .transition_ci_run(&run.id, &[RunState::Running], state, &end)
            .await
        {
            tracing::warn!(run = %run.id, error = %err, "recording a CI run's end failed");
        }
        tracing::info!(run = %run.id, state = state.as_str(), "CI run ended");
        self.ci_publish(&run.id).await;
        // A merge armed while these checks ran happens now (SPEC-011).
        let app = Arc::clone(self);
        let sandbox = run.sandbox_id.clone();
        tokio::spawn(async move { app.merge_armed(&sandbox).await });
    }

    /// Makes sure the run's commit is in the sandbox clone (fetching once if not).
    async fn ci_prepare(&self, sandbox: &SandboxRecord, run: &CiRunRecord) -> Result<(), String> {
        let repo = self.sandbox_repo(&sandbox.id);
        if matches!(
            self.deps.git.resolve_commit(&repo, &run.sha).await,
            Ok(Some(_))
        ) {
            return Ok(());
        }
        if let Err(err) = self.dlv_refresh_clone(sandbox).await {
            return Err(format!(
                "fetching the repository failed: {}",
                err.public_message()
            ));
        }
        match self.deps.git.resolve_commit(&repo, &run.sha).await {
            Ok(Some(_)) => Ok(()),
            _ => Err(format!("commit {} is not in the repository", run.sha)),
        }
    }

    /// Runs one job of a run of `share` jobs: waits for a slot and its CPU and memory budget
    /// (or cancellation; SPEC-082 RES-003), runs it and records the end; `true` when the sandbox
    /// refused it over the disk quota.
    async fn ci_job(
        self: Arc<Self>,
        sandbox: &SandboxRecord,
        run: &CiRunRecord,
        job: &CiJobRecord,
        share: usize,
        checkout: &str,
        mut cancel: watch::Receiver<Option<String>>,
    ) -> bool {
        let store = &self.deps.store;
        let waiting = self.now();
        let slots = self.ci_slots();
        let budget = self.capacity().ci_job(job.need, share);
        let held = async {
            let slot = slots.acquire_owned().await.ok()?;
            let reservation = self.ci_reserve(budget).await?;
            Some((slot, reservation))
        };
        let permit = tokio::select! {
            permit = held => permit,
            reason = wait_cancel(&mut cancel) => {
                let _ = store
                    .transition_ci_job(
                        &job.id,
                        &[RunState::Queued],
                        RunState::Cancelled,
                        &CiJobUpdate {
                            reason: Some(reason),
                            ended_at: Some(self.now()),
                            ..CiJobUpdate::default()
                        },
                    )
                    .await;
                return false;
            }
        };
        let Some(_permit) = permit else {
            return false;
        };
        let started_at = self.now();
        let started = CiJobUpdate {
            started_at: Some(started_at),
            ..CiJobUpdate::default()
        };
        if !matches!(
            store
                .transition_ci_job(&job.id, &[RunState::Queued], RunState::Running, &started)
                .await,
            Ok(true)
        ) {
            return false;
        }
        // Its stages (SPEC-137): the wait for a slot and its budget, then what its output names,
        // under the job's span (the store opened it with the start).
        let mut stages = OutputStages::ci_job(
            run.sandbox_id.clone(),
            &run.id,
            &format!("job:{}@{}", job.id, started_at.0),
            &job.key,
        );
        self.record_span(NewSpan {
            sandbox_id: run.sandbox_id.clone(),
            subject_kind: SubjectKind::CiRun,
            subject_id: run.id.clone(),
            key: format!("wait:{}@{}", job.id, waiting.0),
            parent_key: None,
            kind: "wait".to_owned(),
            name: "job slot".to_owned(),
            environment: None,
            job: Some(job.key.clone()),
            started_at: waiting,
            ended_at: Some(started_at),
            outcome: Some("started".to_owned()),
            detail: Some(
                serde_json::json!({ "cpus": budget.cpus, "memory_mb": budget.memory_mb })
                    .to_string(),
            ),
        })
        .await;
        let mut ring = OutputRing::new(OUTPUT_BYTES);
        ring.push_str(&format!(
            "[yulius] budget: {budget} (YULIUS_CPUS={}, YULIUS_MEMORY_MB={})\n",
            budget.cpus, budget.memory_mb
        ));
        // Its failing tests' output, from all it prints (SPEC-145): the ring keeps only the end.
        let mut failures = FailureScanner::default();
        let (ended, usage) = match self
            .ci_job_process(
                sandbox,
                run,
                job,
                budget,
                checkout,
                (&mut ring, &mut stages, &mut failures),
                &mut cancel,
            )
            .await
        {
            Ok((ended, usage)) => (Ok(ended), usage),
            Err(reason) => (Err(reason), None),
        };
        let no_disk = matches!(ended, Ok(Ended::NoDisk(_)));
        let (state, exit_code, reason) = match ended {
            Ok(Ended::Exit(exit)) if exit.spawn_failed => (
                RunState::Error,
                None,
                Some("the job could not be started".to_owned()),
            ),
            Ok(Ended::Exit(exit)) if exit.is_clean() => (RunState::Passed, Some(0), None),
            // The job said it checked nothing (SPEC-162 SKP-001): skipped, not passed or failed.
            Ok(Ended::Exit(exit))
                if exit.code == Some(dom::SKIP_EXIT_CODE) && exit.signal.is_none() =>
            {
                (RunState::Skipped, exit.code, None)
            }
            Ok(Ended::Exit(exit)) => {
                let code = exit.code.or_else(|| exit.signal.map(|signal| 128 + signal));
                (RunState::Failed, code, None)
            }
            Ok(Ended::Cancelled(reason)) => (RunState::Cancelled, None, Some(reason)),
            Ok(Ended::TimedOut) => (
                RunState::Error,
                None,
                Some(format!(
                    "timed out after {}",
                    dom::format_duration(
                        i64::try_from(self.config.ci.job_timeout.as_millis()).unwrap_or(i64::MAX)
                    )
                )),
            ),
            Ok(Ended::OutputLimit) => (
                RunState::Error,
                None,
                Some("the job wrote more than 256 MiB of output".to_owned()),
            ),
            Ok(Ended::Lost) => (
                RunState::Error,
                None,
                Some("the job's process host was lost".to_owned()),
            ),
            Ok(Ended::NoDisk(reason)) | Err(reason) => (RunState::Error, None, Some(reason)),
        };
        if let Some(reason) = &reason {
            ring.push_str(&format!("\n[yulius] {reason}\n"));
        }
        let end = CiJobUpdate {
            reason,
            exit_code,
            output: Some(ring.text()),
            failures: failures.finish(),
            started_at: None,
            ended_at: Some(self.now()),
            usage,
        };
        if let Err(err) = store
            .transition_ci_job(&job.id, &[RunState::Running], state, &end)
            .await
        {
            tracing::warn!(job = %job.id, error = %err, "recording a CI job's end failed");
        }
        no_disk
    }

    /// The job's environment: no secret; its budget and what the checkout's `[resources.env]`
    /// makes of it (SPEC-082 RES-004).
    fn ci_job_env(
        &self,
        sandbox: &SandboxRecord,
        run: &CiRunRecord,
        job: &CiJobRecord,
        budget: Budget,
        dirs: [&Path; 3],
    ) -> Vec<(String, String)> {
        let [home, cache, checkout] = dirs;
        let path = ci_path(
            &std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into()),
            &self.config.yulius_exe,
        );
        let mut env = vec![
            ("HOME".to_owned(), home.to_string_lossy().into_owned()),
            ("PATH".to_owned(), path),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("TERM".to_owned(), "dumb".to_owned()),
            ("CI".to_owned(), "true".to_owned()),
            ("NO_COLOR".to_owned(), "1".to_owned()),
            ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
            ("YULIUS_CI".to_owned(), "1".to_owned()),
            ("YULIUS_CI_RUN".to_owned(), run.id.clone()),
            ("YULIUS_CI_JOB".to_owned(), job.name.clone()),
            ("YULIUS_CI_SHA".to_owned(), run.sha.clone()),
            ("YULIUS_CI_REF".to_owned(), run.reference.key()),
            (
                "YULIUS_CI_SKIP_CODE".to_owned(),
                dom::SKIP_EXIT_CODE.to_string(),
            ),
            (
                "YULIUS_CI_CACHE".to_owned(),
                cache.to_string_lossy().into_owned(),
            ),
            ("YULIUS_SANDBOX".to_owned(), sandbox.slug.clone()),
            // The base of `yulius security-scan`'s change scan (SPEC-195).
            (
                "YULIUS_INTEGRATION_BRANCH".to_owned(),
                self.profile_of(sandbox).integration_branch,
            ),
        ];
        if let Some(value) = &job.matrix {
            env.push(("YULIUS_CI_MATRIX".to_owned(), value.clone()));
        }
        env.extend(budget_env(budget, &repository_templates(checkout)));
        env
    }

    /// Gives a job the agent secrets the integration branch grants it (SPEC-142 CSE-002..004):
    /// a placeholder for each granted agent secret of the sandbox (never a production-deploy
    /// one) and, when the egress proxy is wired, the proxy with a fresh credential naming client
    /// `ci:<run>` and the CA bundle in `run_dir`, readable inside the containment. Returns the
    /// credential, closed when it is dropped (the job ended, or its task went); `None` without a
    /// grant.
    async fn ci_job_secrets(
        &self,
        sandbox: &SandboxRecord,
        (run, job): (&CiRunRecord, &CiJobRecord),
        run_dir: &Path,
        plan: &mut LaunchPlan,
        ring: &mut OutputRing,
    ) -> Option<CiProxy<'_>> {
        // The sandbox clone (which CI jobs can write) only says whether to look; the grant itself
        // is read from the verified integration head (review B1).
        if granted(self.ci_gate(sandbox).await.gate.as_ref(), &job.key).is_empty() {
            return None;
        }
        let grant = match self.ci_verified_gate(sandbox).await {
            Ok(gate) => granted(gate.as_ref(), &job.key),
            Err(err) => {
                ring.push_str(&format!(
                    "[yulius] agent secrets: none; the integration branch's grant could not be verified: {}\n",
                    err.public_message()
                ));
                return None;
            }
        };
        if grant.is_empty() {
            ring.push_str(
                "[yulius] agent secrets: none; the integration branch grants this job none\n",
            );
            return None;
        }
        let records = match self.applied_agent_secrets(&sandbox.id).await {
            Ok(records) => records,
            Err(err) => {
                tracing::warn!(job = %job.id, error = %err, "agent secrets could not be read for a CI job");
                Vec::new()
            }
        };
        let (mut given, mut missing, mut production) = (Vec::new(), Vec::new(), Vec::new());
        for name in &grant {
            let record = records.iter().find(|r| &r.name == name);
            match (record, SecretName::parse(name)) {
                (Some(record), _) if record.cd_prod => production.push(name.as_str()),
                (Some(record), Ok(parsed)) => {
                    plan.env.push((name.clone(), placeholder(&parsed)));
                    given.push(if record.id == crate::vidoc_key::VIDOC_SECRET_ID {
                        "VIDOC_API_KEY (the instance's Vidoc key)"
                    } else {
                        name.as_str()
                    });
                }
                _ => missing.push(name.as_str()),
            }
        }
        let mut note = format!("[yulius] agent secrets: {}", given.join(", "));
        if given.is_empty() {
            note = "[yulius] agent secrets: none".to_owned();
        }
        if !missing.is_empty() {
            note.push_str(&format!(
                " (not in the sandbox's agent secrets: {})",
                missing.join(", ")
            ));
        }
        if !production.is_empty() {
            note.push_str(&format!(
                " (production-deploy secrets are never given to CI: {})",
                production.join(", ")
            ));
        }
        let token = self
            .ci_egress_open(&sandbox.id, &run.id, &job.id, &grant)
            .map(|token| CiProxy { app: self, token });
        let proxied = token
            .as_ref()
            .is_some_and(|proxy| self.egress_env("yulius-ci", &proxy.token, run_dir, plan));
        if !proxied {
            note.push_str("; the egress proxy is not available, so no secret reaches a host");
        }
        ring.push_str(&format!("{note}\n"));
        bind_run_files(plan);
        token.filter(|_| proxied)
    }

    /// The job's containment: the sandbox's holder when it is isolated (SPEC-002 ADR-22: the
    /// job sees the packages the sandbox installed and its private network), else the previous
    /// containment.
    pub(super) fn ci_containment(
        &self,
        sandbox: &SandboxRecord,
        home: &Path,
        join: Option<SandboxJoin>,
    ) -> Containment {
        if !self.config.contained {
            return Containment::None;
        }
        Containment::Bubblewrap {
            sandbox_root: self.sandbox_dir(&sandbox.id),
            home: home.to_path_buf(),
            writable: Vec::new(),
            // The binary itself, for `yulius security-scan` (SPEC-195), as agent runs see it.
            readonly: std::iter::once(self.config.yulius_exe.clone())
                .chain(self.config.contained_readonly.iter().cloned())
                .chain(join.iter().flat_map(|j| j.extra_readonly.iter().cloned()))
                .collect(),
            binds: Vec::new(),
            identity: None,
            join: join.map(|join| Box::new(ci_join(join))),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn ci_job_process(
        &self,
        sandbox: &SandboxRecord,
        run: &CiRunRecord,
        job: &CiJobRecord,
        budget: Budget,
        checkout_name: &str,
        (ring, stages, failures): (&mut OutputRing, &mut OutputStages, &mut FailureScanner),
        cancel: &mut watch::Receiver<Option<String>>,
    ) -> Result<(Ended, Used), String> {
        let ci_dir = self.ci_dir(&sandbox.id);
        let checkout = ci_dir.join("jobs").join(checkout_name);
        let (home, cache, logs) = (
            ci_dir.join("home"),
            ci_dir.join("cache"),
            ci_dir.join("logs"),
        );
        for dir in [&home, &cache, &logs] {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("preparing the CI directory failed: {e}"))?;
        }
        let repo = self.sandbox_repo(&sandbox.id);
        let checking_out = self.now();
        let checked_out = self
            .deps
            .git
            .ci_checkout(&repo, &checkout, &run.sha, &ci_dir)
            .await;
        let span = stages.child(
            "checkout",
            "prepare",
            "checkout",
            (checking_out, self.now()),
            if checked_out.is_ok() {
                "passed"
            } else {
                "error"
            },
        );
        self.record_span(span).await;
        checked_out
            .map_err(|err| format!("checking out {} failed: {}", run.sha, err.public_message()))?;
        let log = logs.join(format!("{}.log", job.id));
        let _ = std::fs::remove_file(&log);
        std::fs::File::create(&log).map_err(|e| format!("creating the job log failed: {e}"))?;
        // Held until the job ends: the sandbox's holder is not stopped as idle meanwhile. A
        // refusal while the reading is over the quota is the quota: the run waits for disk
        // instead (CI-303). Any other refusal (isolation still being prepared) is an error.
        let (join, _lease) = match self.isolation_join(sandbox).await {
            Ok(joined) => joined.unzip(),
            Err(AppError::Conflict(reason)) if self.ci_over_quota(sandbox).await => {
                return Ok((Ended::NoDisk(reason), None));
            }
            Err(err) => return Err(err.public_message()),
        };
        let mut plan = LaunchPlan {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                WRAPPER.into(),
                "yulius-ci".into(),
                log.to_string_lossy().into_owned(),
                job.command.clone(),
            ],
            cwd: checkout.clone(),
            env: self.ci_job_env(sandbox, run, job, budget, [&home, &cache, &checkout]),
            secret_env: Vec::new(),
            files: Vec::new(),
            containment: self.ci_containment(sandbox, &home, join),
        };
        let run_dir = self.ci_run_dir(&job.id);
        let proxy = self
            .ci_job_secrets(sandbox, (run, job), &run_dir, &mut plan, ring)
            .await;
        let _ = std::fs::remove_dir_all(&run_dir);
        std::fs::create_dir_all(&run_dir)
            .map_err(|e| format!("creating the run directory failed: {e}"))?;
        let token = self.deps.crypto.random_bytes();
        let outcome = self
            .ci_watch(
                &run_dir,
                (&job.id, &job.run_id),
                Sink::CiJob(&job.id),
                &plan,
                &token,
                &log,
                (ring, stages, Some(failures)),
                cancel,
            )
            .await;
        // The job's process ended (or its host is gone): its proxy credential ends too.
        drop(proxy);
        let _ = std::fs::remove_dir_all(&run_dir);
        let _ = std::fs::remove_file(&log);
        outcome
    }

    /// Starts `plan` through the process host as run `ids` (its id and its group) and follows
    /// it to its end: its log into `ring`, `sink` and `failures` (a CI job's), cancellation, the
    /// CI job timeout and the output limit. Returns how it ended and what it used.
    #[allow(clippy::too_many_arguments)] // One job's whole context; split further it only moves.
    pub(super) async fn ci_watch(
        &self,
        run_dir: &Path,
        ids: (&str, &str),
        sink: Sink<'_>,
        plan: &LaunchPlan,
        token: &[u8; 32],
        log: &Path,
        (ring, stages, mut failures): (
            &mut OutputRing,
            &mut OutputStages,
            Option<&mut FailureScanner>,
        ),
        cancel: &mut watch::Receiver<Option<String>>,
    ) -> Result<(Ended, Used), String> {
        let (sandbox, subject) = stages.subject();
        let host = &self.deps.session_host;
        host.spawn(run_dir, ids.0, ids.1, plan, token, (160, 48))
            .await
            .map_err(|err| format!("the job could not be started: {}", err.public_message()))?;
        let attached = match host.attach(run_dir, token).await {
            Ok(attached) => attached,
            Err(err) => {
                let _ = host.terminate(run_dir);
                return Err(format!(
                    "the job's process host did not answer: {}",
                    err.public_message()
                ));
            }
        };
        let (mut reader, mut writer) = (attached.reader, attached.writer);
        let mut tail = LogTail::new(log.to_path_buf());
        let deadline = tokio::time::Instant::now() + self.config.ci.job_timeout;
        let mut tick = tokio::time::interval(READ_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut ticks = 0u32;
        let mut stop: Option<Ended> = None;
        let mut usage: Used = None;
        let ended = loop {
            tokio::select! {
                event = reader.next() => match event {
                    Ok(Some(RunEvent::Exit(exit))) => {
                        usage = exit.usage.filter(|_| stop.is_none());
                        break stop.unwrap_or(Ended::Exit(exit));
                    }
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => break match host.inspect(run_dir) {
                        RunInspection::Exited(exit) => {
                            usage = exit.usage.filter(|_| stop.is_none());
                            stop.unwrap_or(Ended::Exit(exit))
                        }
                        _ => {
                            let _ = host.terminate(run_dir);
                            stop.unwrap_or(Ended::Lost)
                        }
                    },
                },
                reason = wait_cancel(cancel), if stop.is_none() => {
                    let _ = writer.signal(RunSignal::Kill).await;
                    stop = Some(Ended::Cancelled(reason));
                }
                () = tokio::time::sleep_until(deadline), if stop.is_none() => {
                    let _ = writer.signal(RunSignal::Kill).await;
                    stop = Some(Ended::TimedOut);
                }
                () = tokio::time::sleep(KILL_GRACE), if stop.is_some() => {
                    // A killed job whose exit is not seen: its host goes (it reaps the group).
                    let _ = host.terminate(run_dir);
                    break stop.take().unwrap_or(Ended::Lost);
                }
                _ = tick.tick() => {
                    match tail.read(ring).await {
                        Ok(size) if size > MAX_LOG_FILE_BYTES && stop.is_none() => {
                            let _ = writer.signal(RunSignal::Kill).await;
                            stop = Some(Ended::OutputLimit);
                        }
                        _ => {}
                    }
                    let fresh = tail.take_fresh();
                    if let Some(failures) = failures.as_deref_mut() {
                        failures.push(&fresh);
                    }
                    let ops = stages.feed(&fresh, self.now());
                    self.record_spans(&sandbox, (subject.0, &subject.1), ops).await;
                    ticks += 1;
                    if ticks.is_multiple_of(FLUSH_EVERY_TICKS) {
                        match sink {
                            Sink::CiJob(id) => {
                                let _ = self.deps.store.set_ci_job_output(id, &ring.text()).await;
                            }
                            Sink::CdRun(id, sandbox) => {
                                let shown = self.redact_for_sandbox(sandbox, &ring.text()).await;
                                let _ = self.deps.store.set_cd_run_output(id, &shown).await;
                            }
                        }
                    }
                }
            }
        };
        // What the job wrote last (bounded: one more read of at most a chunk per pass).
        for _ in 0..8 {
            let read = tail.read(ring).await;
            let fresh = tail.take_fresh();
            if let Some(failures) = failures.as_deref_mut() {
                failures.push(&fresh);
            }
            let ops = stages.feed(&fresh, self.now());
            self.record_spans(&sandbox, (subject.0, &subject.1), ops)
                .await;
            match read {
                Ok(_) if tail.caught_up() => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let ops = stages.finish(self.now());
        self.record_spans(&sandbox, (subject.0, &subject.1), ops)
            .await;
        Ok((ended, usage))
    }
}

fn first_reason(jobs: &[CiJobRecord], state: RunState) -> Option<String> {
    jobs.iter()
        .filter(|job| job.state == state)
        .find_map(|job| {
            job.reason
                .clone()
                .map(|reason| format!("{}: {reason}", job.name))
        })
}

/// Resolves once the run is cancelled, with the reason.
async fn wait_cancel(cancel: &mut watch::Receiver<Option<String>>) -> String {
    loop {
        if let Some(reason) = cancel.borrow().clone() {
            return reason;
        }
        if cancel.changed().await.is_err() {
            // The sender is gone (the run ended): never resolves.
            std::future::pending::<()>().await;
        }
    }
}

/// Follows a job's log file.
struct LogTail {
    path: PathBuf,
    offset: u64,
    size: u64,
    strip: AnsiStripper,
    /// What the reads since the last [`LogTail::take_fresh`] added (escape sequences removed),
    /// for the job's stages; bounded by the reads' chunks.
    fresh: Vec<u8>,
}

impl LogTail {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            size: 0,
            strip: AnsiStripper::default(),
            fresh: Vec::new(),
        }
    }

    /// The output read since the last call.
    fn take_fresh(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.fresh)
    }

    fn caught_up(&self) -> bool {
        self.offset >= self.size
    }

    /// Reads at most [`READ_CHUNK`] new bytes into `ring`; returns the file size.
    async fn read(&mut self, ring: &mut OutputRing) -> std::io::Result<u64> {
        let mut file = tokio::fs::File::open(&self.path).await?;
        self.size = file.metadata().await?.len();
        if self.size <= self.offset {
            return Ok(self.size);
        }
        // Skip what the ring would discard anyway when far behind.
        let behind = self.size - self.offset;
        let keep = u64::try_from(OUTPUT_BYTES).unwrap_or(u64::MAX);
        if behind > keep.saturating_mul(4) {
            let skipped = behind - keep;
            ring.push_str(&format!("\n[yulius] {skipped} bytes of output skipped\n"));
            self.offset += skipped;
        }
        file.seek(std::io::SeekFrom::Start(self.offset)).await?;
        let want = usize::try_from(self.size - self.offset)
            .unwrap_or(READ_CHUNK)
            .min(READ_CHUNK);
        let mut buf = vec![0u8; want];
        let mut filled = 0;
        while filled < want {
            let n = file.read(&mut buf[filled..]).await?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        self.offset += u64::try_from(filled).unwrap_or(0);
        let mut clean = Vec::with_capacity(filled);
        self.strip.push(&buf[..filled], &mut clean);
        ring.push(&clean);
        self.fresh.extend_from_slice(&clean);
        Ok(self.size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-195 VSC-001: a CI job finds `yulius` (for `yulius security-scan`) on its PATH.
    #[test]
    fn a_ci_job_finds_the_yulius_binary_on_its_path() {
        let exe = Path::new("/opt/yulius/bin/yulius");
        assert_eq!(
            ci_path("/usr/local/bin:/usr/bin:/bin", exe),
            "/opt/yulius/bin:/usr/local/bin:/usr/bin:/bin"
        );
        assert_eq!(
            ci_path("/usr/bin:/opt/yulius/bin:/bin", exe),
            "/usr/bin:/opt/yulius/bin:/bin",
            "already there: unchanged"
        );
        assert_eq!(ci_path("", exe), "/opt/yulius/bin");
        assert_eq!(ci_path("/usr/bin", Path::new("yulius")), "/usr/bin");
    }

    /// SPEC-059 CI-303: only a run whose every error is a disk refusal, with nothing failed,
    /// goes back to the queue; a timeout or a failure next to a refusal keeps its verdict.
    #[test]
    fn only_disk_refusals_put_a_run_back_in_the_queue() {
        use RunState::{Cancelled, Error, Failed, Passed};
        assert!(requeue_for_disk(&[Error, Cancelled, Passed], 1));
        assert!(requeue_for_disk(&[Error, Error], 2));
        assert!(!requeue_for_disk(&[Error, Passed], 0), "no refusal");
        assert!(
            !requeue_for_disk(&[Error, Error], 1),
            "another job errored for real"
        );
        assert!(!requeue_for_disk(&[Error, Failed], 1), "a job failed");
    }

    fn strip(chunks: &[&[u8]]) -> String {
        let mut stripper = AnsiStripper::default();
        let mut out = Vec::new();
        for chunk in chunks {
            stripper.push(chunk, &mut out);
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn escapes_are_removed_across_chunks() {
        assert_eq!(strip(&[b"\x1b[1;31mred\x1b[0m plain"]), "red plain");
        assert_eq!(strip(&[b"a\x1b[", b"32", b"mb"]), "ab");
        assert_eq!(strip(&[b"\x1b]0;title\x07x"]), "x");
        assert_eq!(strip(&[b"\x1b]8;;http://x\x1b\\link"]), "link");
        assert_eq!(strip(&[b"one\r\ntwo\rthree\n"]), "one\ntwo\nthree\n");
        assert_eq!(strip(&[b"bell\x07\x08!"]), "bell!");
    }

    #[test]
    fn a_ci_job_joins_the_ci_group_instead_of_the_agents_group() {
        let join = SandboxJoin {
            holder_pid: 7,
            user_ns: 1,
            net_ns: 2,
            mnt_ns: 3,
            cgroup: Some("/sys/fs/cgroup/u/sandboxes/s1/agents".into()),
            ci_cgroup: Some("/sys/fs/cgroup/u/sandboxes/s1/ci".into()),
            system_root: "/root".into(),
            hostname: "app".into(),
            exec_shim: None,
            extra_readonly: Vec::new(),
            private_dirs: Vec::new(),
            system_passwd: None,
            system_group: None,
            host_system: None,
            docker_dir: Some("/data/runtime/sandboxes/s1/docker".into()),
        };
        let ci = ci_join(join.clone());
        assert_eq!(ci.cgroup, join.ci_cgroup);
        assert_eq!(ci.holder_pid, 7, "the rest of the join is kept");
        // SPEC-187 DKR-001: a CI job uses the sandbox's Docker engine like an agent.
        assert_eq!(ci.docker_dir, join.docker_dir);
        // Without delegation there is no group at all, as before.
        let bare = SandboxJoin {
            cgroup: None,
            ci_cgroup: None,
            ..join
        };
        assert_eq!(ci_join(bare.clone()), bare);
    }

    /// SPEC-142 CSE-002: only the integration branch's `[[ci.jobs]]` entry of the same name
    /// grants secrets; the profile's gate and a pull request's own jobs grant nothing.
    #[test]
    fn only_the_integration_branch_jobs_grant_secrets() {
        let job = |key: &str, secrets: &[&str]| dom::Job {
            name: key.into(),
            key: key.into(),
            command: "x".into(),
            matrix: None,
            need: yulius_domain::resources::Need::default(),
            secrets: secrets.iter().map(|s| (*s).to_owned()).collect(),
        };
        let gate = |source| Gate {
            jobs: vec![job("scan", &["VIDOC_API_KEY"]), job("lint", &[])],
            source,
        };
        let project = gate(GateSource::ProjectConfig);
        assert_eq!(granted(Some(&project), "scan"), ["VIDOC_API_KEY"]);
        assert!(granted(Some(&project), "lint").is_empty());
        assert!(granted(Some(&project), "other").is_empty());
        assert!(granted(Some(&gate(GateSource::Profile)), "scan").is_empty());
        assert!(granted(Some(&gate(GateSource::PullRequest)), "scan").is_empty());
        assert!(granted(None, "scan").is_empty());
    }

    /// SPEC-142 CSE-003: the run's files outside the sandbox (the CA bundle) are readable
    /// inside the containment; the sandbox's own are already there.
    #[test]
    fn run_files_outside_the_sandbox_are_bound_read_only() {
        use crate::ports::session::PlannedFile;
        let file = |path: &str| PlannedFile {
            path: path.into(),
            contents: Vec::new(),
            mode: 0o600,
        };
        let mut plan = LaunchPlan {
            argv: Vec::new(),
            cwd: "/data/sandboxes/s1/ci/jobs/scan".into(),
            env: Vec::new(),
            secret_env: Vec::new(),
            files: vec![
                file("/data/runs/ci-j1/egress-ca.pem"),
                file("/data/sandboxes/s1/x"),
            ],
            containment: Containment::Bubblewrap {
                sandbox_root: "/data/sandboxes/s1".into(),
                home: "/data/sandboxes/s1/ci/home".into(),
                writable: Vec::new(),
                readonly: vec!["/usr".into()],
                binds: Vec::new(),
                identity: None,
                join: None,
            },
        };
        bind_run_files(&mut plan);
        bind_run_files(&mut plan);
        let Containment::Bubblewrap { readonly, .. } = &plan.containment else {
            panic!("contained");
        };
        assert_eq!(
            readonly,
            &[
                PathBuf::from("/usr"),
                PathBuf::from("/data/runs/ci-j1/egress-ca.pem")
            ]
        );
        let mut open = LaunchPlan {
            containment: Containment::None,
            ..plan
        };
        bind_run_files(&mut open);
        assert_eq!(open.containment, Containment::None);
    }

    #[test]
    fn matrix_siblings_get_their_own_checkouts() {
        let job = |id: &str, key: &str| CiJobRecord {
            id: id.into(),
            run_id: "r".into(),
            ordinal: 0,
            name: key.into(),
            key: key.into(),
            command: "x".into(),
            matrix: None,
            state: RunState::Queued,
            reason: None,
            exit_code: None,
            output: String::new(),
            failures: None,
            started_at: None,
            ended_at: None,
            need: yulius_domain::resources::Need::default(),
        };
        let jobs = vec![job("a", "lint"), job("b", "test"), job("c", "test")];
        assert_eq!(checkout_name(&jobs[0], &jobs, 0), "lint");
        assert_eq!(checkout_name(&jobs[1], &jobs, 0), "test.1");
        assert_eq!(checkout_name(&jobs[2], &jobs, 0), "test.2");
        // A second lane of the sandbox has checkouts of its own.
        assert_eq!(checkout_name(&jobs[0], &jobs, 1), "lint@1");
        assert_eq!(checkout_name(&jobs[2], &jobs, 1), "test.2@1");
    }
}
