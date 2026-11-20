//! The merge-first staging queue (SPEC-575 MFS-003 to MFS-008, ADR-0411).
//!
//! A project's shared staging target is the dev-stage `[cd.<name>]` with `merge_first = true`
//! at the verified integration head. Nothing deploys it directly:
//!
//! 1. **Request.** A task's agent (`cd_deploy`) or a person (the CD page) asks for it, and Yulius
//!    records a staging request for a revision. For a task that is its newest landing on the
//!    integration branch; for a person, the verified integration head.
//! 2. **Candidate gate.** The full gate must pass on exactly that revision. A branch's or
//!    candidate's run that already judged the commit counts; otherwise a
//!    [`CiRef::Candidate`] run is queued.
//! 3. **Deploy in revision order.** One deploy per environment at a time, of the newest ready
//!    revision above the last verified one. Requests it contains are included in it.
//! 4. **Outcome.** A passed run (deploy and verify) is verified; a failed one is the task's to
//!    fix. A run lost to a restart or ended in error is tried again, up to
//!    [`MAX_DEPLOY_ATTEMPTS`].
//!
//! Every move is a compare-and-set from the state it was decided on
//! ([`crate::ports::store_ext::StagingStore`]), and the decisions are
//! [`yulius_domain::staging`]'s. A second tick, a duplicate request or a restart therefore
//! never deploys a request twice.

use std::sync::Arc;

use yulius_domain::cd::{CdStage, CdTarget, Requester};
use yulius_domain::ci::{CiRef, RunState};
use yulius_domain::staging::{
    self as stg, Admission, DeployEnd, Facts, MAX_DEPLOY_ATTEMPTS, StagingState,
};

use super::cd::{CdActor, CdRequest, mirror_lock};
use crate::context::App;
use crate::coordination::AgentContext;
use crate::delivery::delivery_profile;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::store_ext::{StagingChange, StagingRequestRecord};

/// Most requests a queue reads at once (the requests still on their way).
const MAX_PENDING: usize = 200;

/// One lock per sandbox around its staging queue's decisions.
fn queue_lock(sandbox: &yulius_domain::ids::SandboxId) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    let locks = LOCKS.get_or_init(Default::default);
    let mut map = locks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.entry(sandbox.as_str().to_owned()).or_default().clone()
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

impl App {
    /// The verified integration head and its merge-first environment, if it has one: read from
    /// the CD mirror, never from anything an agent can write.
    pub(crate) async fn staging_target(
        &self,
        sandbox: &SandboxRecord,
    ) -> AppResult<(String, Option<CdTarget>)> {
        let lock = mirror_lock(&sandbox.id);
        let _mirror = lock.lock().await;
        let profile = delivery_profile(sandbox);
        self.dlv_refresh_clone(sandbox).await?;
        let token = self.dlv_installation_token(sandbox).await?;
        let head = self
            .cd_verified_head(sandbox, &profile.integration_branch, &token)
            .await?;
        let config = self.cd_verified_config(sandbox, &head).await?;
        Ok((head, config.merge_first().cloned()))
    }

    /// Records the staging request of `env` for a task's agent (`cd_deploy`; SPEC-575 MFS-004,
    /// MFS-005): its newest landing on the integration branch. A repeated request answers with
    /// the one recorded.
    ///
    /// # Errors
    /// `Conflict` for a request that cannot be served (no landing, open steps, no CI, already
    /// deployed once, a deploy of the task running); store and Git failures.
    pub(crate) async fn staging_request(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        env: &str,
        ctx: &AgentContext,
        head: &str,
    ) -> AppResult<StagingRequestRecord> {
        let profile = delivery_profile(sandbox);
        let (task, requested_by, revision) = {
            {
                let open: Vec<String> = self
                    .deps
                    .store
                    .steps(&ctx.task.id)
                    .await?
                    .into_iter()
                    .filter(|s| matches!(s.status.as_str(), "pending" | "blocked"))
                    .map(|s| s.title)
                    .collect();
                if let Some(first) = open.first() {
                    return Err(AppError::Conflict(format!(
                        "staging takes a task's whole delivery once: {} step(s) still open (\"{first}\"); finish and merge them, then ask again",
                        open.len()
                    )));
                }
                let landings = self
                    .deps
                    .store
                    .task_landings(&ctx.task.id, &profile.integration_branch)
                    .await?;
                let Some(revision) = landings.last().cloned() else {
                    return Err(AppError::Conflict(format!(
                        "nothing to deploy yet: this task has merged nothing into {}; staging takes the merged revision, after delivery_merge",
                        profile.integration_branch
                    )));
                };
                (Some(ctx.task.id.clone()), ctx.actor(), revision)
            }
        };
        if self.ci_gate(sandbox).await.gate.is_none() {
            return Err(AppError::Conflict(
                "merge-first staging deploys only a revision whose full gate passed, and this project has no CI gate".into(),
            ));
        }
        let lock = queue_lock(&sandbox.id);
        let _queue = lock.lock().await;
        let store = &self.deps.store;
        let own = store
            .staging_requests_of(&sandbox.id, env, task.as_ref(), &revision)
            .await?;
        let positions = self
            .staging_positions(sandbox, head, own.iter().map(|r| r.revision.as_str()))
            .await;
        let facts: Vec<Facts> = own.iter().map(|r| facts_of(r, &positions)).collect();
        let contained = self.staging_contained(sandbox, env, &revision).await?;
        let now = self.now();
        match stg::admit(&revision, &facts, contained.is_some()) {
            Admission::Existing(id) => store.staging_request(&id).await?.ok_or(AppError::NotFound),
            Admission::Retry(id) => {
                let change = StagingChange {
                    attempts: Some(0),
                    note: Some("asked again after its deploy failed".into()),
                    ..StagingChange::default()
                };
                store
                    .move_staging_request(
                        &id,
                        StagingState::DeployFailed,
                        StagingState::Ready,
                        &change,
                        now,
                    )
                    .await?;
                self.staging_kick(sandbox, env);
                store.staging_request(&id).await?.ok_or(AppError::NotFound)
            }
            Admission::Refused(why) => Err(AppError::Conflict(why)),
            Admission::New { state, supersede } => {
                for id in &supersede {
                    let old = own.iter().find(|r| &r.id == id);
                    if let Some(old) = old {
                        let change = StagingChange {
                            note: Some(format!("superseded by revision {}", short(&revision))),
                            ..StagingChange::default()
                        };
                        store
                            .move_staging_request(
                                id,
                                old.state,
                                StagingState::Superseded,
                                &change,
                                now,
                            )
                            .await?;
                    }
                }
                let record = StagingRequestRecord {
                    id: self.deps.crypto.new_id(),
                    sandbox_id: sandbox.id.clone(),
                    environment: env.to_owned(),
                    task_id: task,
                    revision: revision.clone(),
                    state,
                    ci_run_id: None,
                    cd_run_id: contained.clone(),
                    attempts: 0,
                    note: contained.as_ref().map(|run| {
                        format!("already on staging: run {run} deployed and verified a revision containing it")
                    }),
                    requested_by,
                    told: false,
                    created_at: now,
                    updated_at: now,
                };
                if !store.insert_staging_request(&record).await? {
                    // A concurrent request for the same revision won: answer with it.
                    let again = store
                        .staging_requests_of(&sandbox.id, env, record.task_id.as_ref(), &revision)
                        .await?;
                    return again
                        .into_iter()
                        .find(|r| r.revision == revision && r.state != StagingState::Superseded)
                        .ok_or_else(|| {
                            AppError::Conflict("the staging request could not be recorded".into())
                        });
                }
                self.dlv_audit(
                    &record.requested_by,
                    "staging.requested",
                    &format!("{}/{env}", sandbox.slug),
                    &format!("revision {} ({})", short(&revision), state.as_str()),
                )
                .await;
                self.staging_kick(sandbox, env);
                Ok(record)
            }
        }
    }

    /// Advances one sandbox's queue soon, off the caller's path.
    pub(crate) fn staging_kick(self: &Arc<Self>, sandbox: &SandboxRecord, env: &str) {
        let (app, sandbox, env) = (Arc::clone(self), sandbox.clone(), env.to_owned());
        tokio::spawn(async move {
            if let Err(err) = app.staging_advance(&sandbox, &env).await {
                tracing::warn!(error = %err, sandbox = %sandbox.slug, "the staging queue could not advance");
            }
        });
    }

    /// Advances every queue with a request on its way (the background tick).
    pub async fn staging_tick(self: &Arc<Self>) {
        let queues = match self.deps.store.staging_queues().await {
            Ok(queues) => queues,
            Err(err) => {
                tracing::warn!(error = %err, "staging queues could not be read");
                return;
            }
        };
        for (sandbox_id, env) in queues {
            let Ok(Some(sandbox)) = self.deps.store.sandbox(&sandbox_id).await else {
                continue;
            };
            if sandbox.removed_at.is_some() {
                continue;
            }
            if let Err(err) = self.staging_advance(&sandbox, &env).await {
                tracing::warn!(error = %err, sandbox = %sandbox.slug, "the staging queue could not advance");
            }
        }
    }

    /// One round of a queue (SPEC-575 MFS-003, MFS-006 to MFS-008): verdicts, candidate runs,
    /// the outcome of the running deploy, inclusions and the next deploy.
    ///
    /// # Errors
    /// GitHub, Git and store failures (the round is tried again at the next tick).
    pub async fn staging_advance(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        env: &str,
    ) -> AppResult<()> {
        let lock = queue_lock(&sandbox.id);
        let _queue = lock.lock().await;
        let store = &self.deps.store;
        let mut pending = store.pending_staging_requests(&sandbox.id, env).await?;
        pending.truncate(MAX_PENDING);
        if pending.is_empty() {
            return Ok(());
        }
        let (head, target) = self.staging_target(sandbox).await?;
        if target.as_ref().is_none_or(|t| t.name != env) {
            // The environment is no longer the staging target: nothing it waits for will come.
            for request in pending
                .iter()
                .filter(|r| r.state != StagingState::Deploying)
            {
                self.staging_move(
                    request,
                    StagingState::Superseded,
                    StagingChange {
                        note: Some(format!(
                            "[cd.{env}] is no longer the merge-first staging target"
                        )),
                        ..StagingChange::default()
                    },
                )
                .await?;
            }
            return Ok(());
        }
        let now_pending: Vec<&StagingRequestRecord> = pending.iter().collect();
        self.staging_settle_deploy(&now_pending).await?;
        let mut pending = store.pending_staging_requests(&sandbox.id, env).await?;
        pending.truncate(MAX_PENDING);
        self.staging_judge(sandbox, &pending).await?;
        let pending = store.pending_staging_requests(&sandbox.id, env).await?;
        let positions = self
            .staging_positions(sandbox, &head, pending.iter().map(|r| r.revision.as_str()))
            .await;
        // A revision no longer on the branch's own history never deploys.
        for request in &pending {
            if request.state != StagingState::Deploying
                && positions
                    .iter()
                    .any(|(sha, at)| sha == &request.revision && at.is_none())
            {
                let change = StagingChange {
                    note: Some(format!(
                        "revision {} is no longer on the integration branch's own history",
                        short(&request.revision)
                    )),
                    ..StagingChange::default()
                };
                self.staging_move(request, StagingState::Superseded, change)
                    .await?;
            }
        }
        let (current, verified_run) = self.staging_current(sandbox, env, &head).await?;
        let facts: Vec<Facts> = pending.iter().map(|r| facts_of(r, &positions)).collect();
        let pick = stg::pick(&facts, current);
        for id in &pick.included {
            if let Some(request) = pending.iter().find(|r| &r.id == id) {
                let change = StagingChange {
                    cd_run_id: verified_run.clone(),
                    note: Some(
                        "included: staging already serves a verified revision containing it".into(),
                    ),
                    ..StagingChange::default()
                };
                self.staging_move(request, StagingState::Included, change)
                    .await?;
            }
        }
        let Some(deploy) = pick
            .deploy
            .as_ref()
            .and_then(|id| pending.iter().find(|r| &r.id == id))
        else {
            return Ok(());
        };
        self.staging_start_deploy(sandbox, env, deploy).await
    }

    /// The full gate's verdicts on the testing requests' revisions, and a candidate run for each
    /// revision no run judged or is judging (SPEC-575 MFS-003).
    async fn staging_judge(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        pending: &[StagingRequestRecord],
    ) -> AppResult<()> {
        for request in pending.iter().filter(|r| r.state == StagingState::Testing) {
            let runs = self
                .deps
                .store
                .ci_runs_for_sha(&sandbox.id, &request.revision)
                .await?;
            // Only the merged revision's own runs judge it: a branch's run or a candidate's,
            // never one that skipped the full suite.
            let mut judging = None;
            for run in runs
                .iter()
                .filter(|r| matches!(r.reference, CiRef::Branch(_) | CiRef::Candidate(_)))
            {
                if run.state == RunState::Cancelled {
                    continue;
                }
                if run.state.is_active() {
                    judging = Some(run.clone());
                    break;
                }
                let jobs = self.deps.store.ci_jobs(&run.id, false).await?;
                if jobs
                    .iter()
                    .any(|j| yulius_domain::ci::skipped_after_merge(j.state, j.reason.as_deref()))
                {
                    continue;
                }
                judging = Some(run.clone());
                break;
            }
            match judging {
                Some(run) => {
                    let Some(next) = stg::after_verdict(Some(run.state)) else {
                        continue;
                    };
                    let note = if next == StagingState::GateFailed {
                        let jobs = self.deps.store.ci_jobs(&run.id, false).await?;
                        let failed: Vec<String> = jobs
                            .iter()
                            .filter(|j| matches!(j.state, RunState::Failed | RunState::Error))
                            .map(|j| j.name.clone())
                            .collect();
                        Some(format!(
                            "the full gate failed on revision {}: run {} ({}); staging did not change",
                            short(&request.revision),
                            run.id,
                            if failed.is_empty() {
                                run.state.as_str().to_owned()
                            } else {
                                failed.join(", ")
                            }
                        ))
                    } else {
                        Some(format!("the full gate passed: run {}", run.id))
                    };
                    let change = StagingChange {
                        ci_run_id: Some(run.id.clone()),
                        note,
                        ..StagingChange::default()
                    };
                    self.staging_move(request, next, change).await?;
                }
                None => {
                    let Some(gate) = self.ci_gate(sandbox).await.gate else {
                        continue;
                    };
                    let run = self
                        .ci_queue(
                            sandbox,
                            CiRef::Candidate(request.revision.clone()),
                            &request.revision,
                            &gate,
                            "yulius",
                        )
                        .await?;
                    let change = StagingChange {
                        ci_run_id: Some(run.id.clone()),
                        note: Some(format!("the full gate runs on it: run {}", run.id)),
                        ..StagingChange::default()
                    };
                    self.staging_move(request, StagingState::Testing, change)
                        .await?;
                    self.ci_wake();
                }
            }
        }
        Ok(())
    }

    /// Reads the outcome of the deploy running for the queue (SPEC-575 MFS-007, MFS-008): a
    /// passed run verifies its request, a failed one fails it, an errored or lost one makes it
    /// ready again until its attempts run out.
    async fn staging_settle_deploy(
        self: &Arc<Self>,
        pending: &[&StagingRequestRecord],
    ) -> AppResult<()> {
        for request in pending
            .iter()
            .filter(|r| r.state == StagingState::Deploying)
        {
            let run = match &request.cd_run_id {
                Some(id) => self.deps.store.cd_run(id, false).await?,
                None => None,
            };
            let end = match run.as_ref().map(|r| r.state) {
                Some(RunState::Running | RunState::Queued) => continue,
                Some(RunState::Passed) => DeployEnd::Passed,
                Some(RunState::Failed) => DeployEnd::Failed,
                // No run was recorded (a start lost before its record) or it ended in error
                // (a restart reconciles a running run to an error).
                _ => DeployEnd::Lost,
            };
            let attempts = request.attempts.saturating_add(1);
            let next = stg::after_deploy(end, attempts);
            let run_id = request.cd_run_id.clone().unwrap_or_default();
            let note = match (end, next) {
                (DeployEnd::Passed, _) => format!(
                    "deployed and verified on staging: run {run_id}, revision {}",
                    short(&request.revision)
                ),
                (DeployEnd::Failed, _) => format!(
                    "the deploy or verify failed: run {run_id}; fix it forward, merge, and ask again"
                ),
                (DeployEnd::Lost, StagingState::Ready) => format!(
                    "attempt {attempts} of {MAX_DEPLOY_ATTEMPTS} ended without a verdict (run {}): tried again",
                    if run_id.is_empty() {
                        "not recorded"
                    } else {
                        &run_id
                    }
                ),
                (DeployEnd::Lost, _) => format!(
                    "{MAX_DEPLOY_ATTEMPTS} attempts ended without a verdict; the last was run {run_id}"
                ),
            };
            let change = StagingChange {
                attempts: Some(attempts),
                note: Some(note),
                ..StagingChange::default()
            };
            self.staging_move(request, next, change).await?;
        }
        Ok(())
    }

    /// Starts the deploy of `request`: it moves to deploying first (one per environment, by the
    /// store's rule), then its run starts and is recorded on it.
    async fn staging_start_deploy(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        env: &str,
        request: &StagingRequestRecord,
    ) -> AppResult<()> {
        let store = &self.deps.store;
        // Another run of the environment (a rollback, its drill) runs: the deploy waits for it,
        // costing no attempt.
        if store
            .running_cd_runs()
            .await?
            .iter()
            .any(|run| run.sandbox_id == sandbox.id && run.environment == env)
        {
            return Ok(());
        }
        let moved = store
            .move_staging_request(
                &request.id,
                StagingState::Ready,
                StagingState::Deploying,
                &StagingChange {
                    note: Some(format!("deploying revision {}", short(&request.revision))),
                    ..StagingChange::default()
                },
                self.now(),
            )
            .await?;
        if !moved {
            return Ok(());
        }
        // The deploy acts for the person behind the request: the task's owner, or the person.
        let person = match &request.task_id {
            Some(task) => match store.task(task).await? {
                Some(task) => store.user(&task.owner).await?,
                None => None,
            },
            None => match yulius_domain::ids::UserId::parse(&request.requested_by) {
                Ok(id) => store.user(&id).await?,
                Err(_) => None,
            },
        }
        .filter(|user| user.disabled_at.is_none());
        let started = match &person {
            Some(person) => {
                let note = format!(
                    "merge-first staging: request {} ({})",
                    request.id,
                    match &request.task_id {
                        Some(_) => "a task's delivery",
                        None => "a person's request",
                    }
                );
                self.cd_start(
                    sandbox,
                    env,
                    Requester::Person {
                        administrator: false,
                        confirmation: None,
                    },
                    &CdActor::Person(person),
                    CdRequest::DeployRevision {
                        revision: &request.revision,
                        note: &note,
                    },
                )
                .await
            }
            None => Err(AppError::Forbidden(
                "nobody active can deploy for this request".into(),
            )),
        };
        let now = self.now();
        match started {
            Ok(run) => {
                store
                    .move_staging_request(
                        &request.id,
                        StagingState::Deploying,
                        StagingState::Deploying,
                        &StagingChange {
                            cd_run_id: Some(run.id.clone()),
                            note: Some(format!(
                                "deploying revision {}: run {}",
                                short(&request.revision),
                                run.id
                            )),
                            ..StagingChange::default()
                        },
                        now,
                    )
                    .await?;
            }
            Err(err) => {
                let attempts = request.attempts.saturating_add(1);
                let next = match err {
                    // What only a person can change (access, a secret's allowance) fails now.
                    AppError::Forbidden(_) => StagingState::DeployFailed,
                    _ => stg::after_deploy(DeployEnd::Lost, attempts),
                };
                store
                    .move_staging_request(
                        &request.id,
                        StagingState::Deploying,
                        next,
                        &StagingChange {
                            attempts: Some(attempts),
                            note: Some(format!(
                                "the deploy could not start: {}",
                                err.public_message()
                            )),
                            ..StagingChange::default()
                        },
                        now,
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// The position of the last verified staging revision and the run that verified it: the
    /// newest verified request's, else (before merge-first) the newest passed deploy of the
    /// environment.
    async fn staging_current(
        &self,
        sandbox: &SandboxRecord,
        env: &str,
        head: &str,
    ) -> AppResult<(Option<u64>, Option<String>)> {
        let recent = self
            .deps
            .store
            .recent_staging_requests(&sandbox.id, env, 50)
            .await?;
        let mut best: Option<(u64, Option<String>)> = None;
        let verified: Vec<&StagingRequestRecord> = recent
            .iter()
            .filter(|r| r.state == StagingState::Verified)
            .collect();
        let revisions: Vec<&str> = verified.iter().map(|r| r.revision.as_str()).collect();
        let positions = self
            .staging_positions(sandbox, head, revisions.into_iter())
            .await;
        for request in verified {
            if let Some(Some(at)) = positions
                .iter()
                .find(|(sha, _)| sha == &request.revision)
                .map(|(_, at)| *at)
                && best.as_ref().is_none_or(|(b, _)| at > *b)
            {
                best = Some((at, request.cd_run_id.clone()));
            }
        }
        if best.is_none()
            && let Some(run) = self
                .deps
                .store
                .last_passed_cd_run(&sandbox.id, env, CdStage::Dev)
                .await?
        {
            let positions = self
                .staging_positions(sandbox, head, std::iter::once(run.sha.as_str()))
                .await;
            if let Some((_, Some(at))) = positions.first() {
                best = Some((*at, Some(run.id.clone())));
            }
        }
        Ok(best.map_or((None, None), |(at, run)| (Some(at), run)))
    }

    /// The verified run of `env` whose revision contains `revision`, if one does (SPEC-575
    /// MFS-005: a delivery already on staging, also from before merge-first, is not deployed
    /// twice).
    async fn staging_contained(
        &self,
        sandbox: &SandboxRecord,
        env: &str,
        revision: &str,
    ) -> AppResult<Option<String>> {
        let mirror = self.cd_mirror(&sandbox.id);
        let recent = self
            .deps
            .store
            .recent_staging_requests(&sandbox.id, env, 50)
            .await?;
        let mut candidates: Vec<(String, String)> = recent
            .iter()
            .filter(|r| r.state == StagingState::Verified)
            .filter_map(|r| r.cd_run_id.clone().map(|run| (r.revision.clone(), run)))
            .collect();
        if let Some(run) = self
            .deps
            .store
            .last_passed_cd_run(&sandbox.id, env, CdStage::Dev)
            .await?
        {
            candidates.push((run.sha.clone(), run.id.clone()));
        }
        for (deployed, run) in candidates {
            if self
                .deps
                .git
                .is_ancestor(&mirror, revision, &deployed)
                .await
                .unwrap_or(false)
            {
                return Ok(Some(run));
            }
        }
        Ok(None)
    }

    /// Each revision's first-parent position on the verified integration head (in the CD
    /// mirror); `None` for one off that history or that Git cannot place.
    async fn staging_positions<'a>(
        &self,
        sandbox: &SandboxRecord,
        head: &str,
        revisions: impl Iterator<Item = &'a str>,
    ) -> Vec<(String, Option<u64>)> {
        let mirror = self.cd_mirror(&sandbox.id);
        let mut out: Vec<(String, Option<u64>)> = Vec::new();
        for revision in revisions {
            if out.iter().any(|(sha, _)| sha == revision) {
                continue;
            }
            let at = self
                .deps
                .git
                .first_parent_position(&mirror, head, revision)
                .await
                .ok()
                .flatten();
            out.push((revision.to_owned(), at));
        }
        out
    }

    /// Moves `request` from the state it was read in; a request another round moved meanwhile
    /// is left as it is.
    async fn staging_move(
        &self,
        request: &StagingRequestRecord,
        to: StagingState,
        change: StagingChange,
    ) -> AppResult<bool> {
        let moved = self
            .deps
            .store
            .move_staging_request(&request.id, request.state, to, &change, self.now())
            .await?;
        if moved && to != request.state {
            tracing::info!(request = %request.id, from = request.state.as_str(), to = to.as_str(), revision = %short(&request.revision), "staging request moved");
        }
        Ok(moved)
    }
}

fn facts_of(request: &StagingRequestRecord, positions: &[(String, Option<u64>)]) -> Facts {
    Facts {
        id: request.id.clone(),
        task: request.task_id.as_ref().map(|t| t.as_str().to_owned()),
        revision: request.revision.clone(),
        state: request.state,
        position: positions
            .iter()
            .find(|(sha, _)| sha == &request.revision)
            .and_then(|(_, at)| *at),
        attempts: request.attempts,
    }
}
