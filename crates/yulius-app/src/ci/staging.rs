//! The merge-first staging queue (SPEC-575 MFS-003 to MFS-009, ADR-0411).
//!
//! A project's shared staging target is the dev-stage `[cd.<name>]` with `merge_first = true`
//! at the verified integration head. Nothing deploys its head directly:
//!
//! 1. **Request.** A task's agent asks with `cd_deploy` under its owner's write access, and
//!    Yulius records a staging request for the task's newest landing on the integration
//!    branch.
//! 2. **Candidate gate.** The full gate must pass on exactly that revision. The integration
//!    branch's run or a candidate's run that judged the commit counts; otherwise one
//!    [`CiRef::Candidate`] run is queued, which the branch moving on never supersedes.
//! 3. **Deploy in revision order.** One deploy per environment at a time, of the newest ready
//!    revision above what staging serves. It waits, costing no attempt, while another run of
//!    the environment goes on or an update holds deploys. Requests staging contains are
//!    included in it.
//! 4. **Outcome.**
//!    - A passed run (deploy and verify) is verified.
//!    - A failed deploy is recorded and staging is restored to its last verified release.
//!    - A person's cancel stands.
//!    - A run lost to a restart or ended in error is tried again, up to [`MAX_DEPLOY_ATTEMPTS`].
//!    - The outcome remains on the delivery. A running task with work left also hears it once.
//!
//! Every move is a compare-and-set from the state it was decided on
//! ([`crate::ports::store_ext::StagingStore`]), and the decisions are
//! [`yulius_domain::staging`]'s. A second tick, a duplicate request or a restart therefore
//! never deploys a request twice.

use std::sync::Arc;
use std::time::Duration;

use yulius_domain::cd::{CdStage, CdTarget, Requester};
use yulius_domain::ci::{CiRef, RunState};
use yulius_domain::rollback::CdRunKind;
use yulius_domain::staging::{
    self as stg, Admission, DeployEnd, Facts, MAX_DEPLOY_ATTEMPTS, StagingState,
};
use yulius_domain::staging_gate as judge;
use yulius_domain::task::{Activity, TaskStatus};

use super::cd::{CdActor, CdRequest, mirror_lock};
use yulius_domain::permission::Action;

use crate::context::App;
use crate::coordination::AgentContext;
use crate::delivery::delivery_profile;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, UserRecord};
use crate::ports::store_ext::{StagingChange, StagingRequestRecord};
use crate::task_message::{Sender, Typed};

/// Most requests a queue reads at once (the requests still on their way).
const MAX_PENDING: usize = 200;
/// Most first-parent revisions below the verified head the full gate's judging reads in one
/// round (SPEC-653 NWF-007).
const WINDOW: usize = 512;
/// A task view or browser handoff that is busy yields to the staging queue.
pub(super) const TASK_VIEW_BUDGET: Duration = Duration::from_millis(750);

/// What a finished task whose background work waits for its staging outcome is told after it
/// (SPEC-631 SWK-001): that work keeps it from Done.
const AWAITED_OUTCOME: &str = "This task's work is finished: if a background command of yours \
     waits for this outcome, stop it, and leave anything else running; then answer in one line \
     and end your turn.";

/// How a person's request names its requester (SPEC-650 PST-003): `user:<id>`.
pub(crate) const PERSON_REQUESTER: &str = "user:";
/// The schedule's requests' requester (SPEC-650 PST-006).
pub(crate) const SCHEDULE_REQUESTER: &str = "yulius:schedule";

/// How a staging deploy's run note starts: the queue knows its own runs by it.
const DEPLOY_NOTE: &str = "merge-first staging: request";
/// How a restore's run note starts, before the part's label.
const RESTORE_NOTE: &str = "merge-first staging: restoring the last verified release of";

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

pub(super) fn short(sha: &str) -> &str {
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
        let branch = &profile.integration_branch;
        let token = self.dlv_installation_token(sandbox).await?;
        // The clone usually has the head already; it is fetched only when it does not, as for
        // the CI gate (`ci_verified_gate`).
        let head = match self.cd_verified_head(sandbox, branch, &token).await {
            Ok(head) => head,
            Err(_) => {
                self.dlv_refresh_clone(sandbox).await?;
                self.cd_verified_head(sandbox, branch, &token).await?
            }
        };
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
        // Under its owner's write access, as any deploy for a task (review round 1, B1).
        if self.staging_owner(&ctx.task).await?.is_none() {
            return Err(AppError::Forbidden(
                "staging deploys for a task under its owner's write access, and the person who owns this task has none (or is no longer active)".into(),
            ));
        }
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
                let landed = self
                    .deps
                    .store
                    .staging_landing(&ctx.task.id, &profile.integration_branch, None)
                    .await?;
                let Some(revision) = landed else {
                    return Err(AppError::Conflict(format!(
                        "nothing to deploy yet: this task has merged nothing confirmed for its current delivery into {}; read delivery_status to reconcile the landing before asking for staging",
                        profile.integration_branch
                    )));
                };
                (Some(ctx.task.id.clone()), ctx.actor(), revision)
            }
        };
        if !self.config.ci.enabled || self.ci_gate(sandbox).await.gate.is_none() {
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
            .await?;
        let facts: Vec<Facts> = own.iter().map(|r| facts_of(r, &positions)).collect();
        let Some(target) = self
            .staging_target(sandbox)
            .await?
            .1
            .filter(|t| t.name == env)
        else {
            return Err(AppError::Conflict(format!(
                "[cd.{env}] is no longer the merge-first staging target"
            )));
        };
        let contained = self.staging_contained(sandbox, &target, &revision).await?;
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
                        let moved = store
                            .move_staging_request(
                                id,
                                old.state,
                                StagingState::Superseded,
                                &change,
                                now,
                            )
                            .await?;
                        // The agent asked for the newer revision itself: nothing to tell it.
                        if moved && let Some(outcome) = store.staging_request(id).await? {
                            store.mark_staging_told(&outcome).await?;
                        }
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

    /// A person's request for the merge-first environment `env` (SPEC-650 PST-001): the CD page's
    /// Deploy, or their Superagent's `cd_deploy` once approved on its panel. It records a request
    /// of no task for the verified integration head; the queue deploys it once its full gate
    /// passed, or the next green revision when it is red. A repeat answers with the request.
    ///
    /// # Errors
    /// `Forbidden` without write access; `Conflict` for an environment that is not the
    /// merge-first target, a project without a CI gate, a playground; store and Git failures.
    pub(crate) async fn staging_ask(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        env: &str,
        user: &UserRecord,
    ) -> AppResult<StagingRequestRecord> {
        crate::playground::refuse_in_playground(sandbox, "A deploy")?;
        if self.staging_writer(&sandbox.id, &user.id).await?.is_none() {
            return Err(AppError::Forbidden(
                "asking for staging needs write access to this project".into(),
            ));
        }
        if !self.config.ci.enabled || self.ci_gate(sandbox).await.gate.is_none() {
            return Err(AppError::Conflict(
                "merge-first staging deploys only a revision whose full gate passed, and this project has no CI gate".into(),
            ));
        }
        crate::ports::github::for_purpose(crate::ports::github::GitHubPurpose::Staging, async {
            let (head, target) = self.staging_target(sandbox).await?;
            let Some(target) = target.filter(|t| t.name == env) else {
                return Err(AppError::Conflict(format!(
                    "[cd.{env}] is not the merge-first staging target: deploy it with its Deploy button or cd_deploy"
                )));
            };
            let lock = queue_lock(&sandbox.id);
            let _queue = lock.lock().await;
            let requester = format!("{PERSON_REQUESTER}{}", user.id.as_str());
            self.staging_record_task_less(sandbox, &target, &head, &requester, Asking::Person)
                .await?
                .ok_or_else(|| AppError::internal("a person's request is always answered"))
        })
        .await
    }

    /// Records a request of no task for `revision` of the integration branch (SPEC-650): a
    /// person's, the schedule's, or a red one's follow-up (`Asking::Yulius`). The caller holds
    /// the queue's lock.
    ///
    /// - A pending request of a person or the schedule for that revision answers instead.
    /// - One that succeeded answers while staging still contains the revision.
    /// - Otherwise (it failed, or staging was rolled back past it) a new request is recorded,
    ///   for the one asking now (review round 1, findings 3 and 4): a failed one is never made
    ///   ready again, so a person's cancel stands.
    /// - Yulius's own ask records nothing when that revision has any request (a task's too) in
    ///   any state but superseded
    ///   (finding 1): `None`.
    async fn staging_record_task_less(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        target: &CdTarget,
        revision: &str,
        requested_by: &str,
        asking: Asking,
    ) -> AppResult<Option<StagingRequestRecord>> {
        let store = &self.deps.store;
        let env = target.name.as_str();
        let own: Vec<StagingRequestRecord> = store
            .staging_requests_of(&sandbox.id, env, None, revision)
            .await?
            .into_iter()
            .filter(|r| is_asked(r) && r.state != StagingState::Superseded)
            .collect();
        let note = match asking {
            Asking::Yulius(_) if !own.is_empty() => return Ok(None),
            Asking::Yulius(note) => {
                // Any request of that revision, a task's too, was tried already: a cancel, a
                // rollback or a failed deploy stands until a new merge or a person asks (review
                // round 1 of PR 2). An inherited one only waits: a rerun may still green it.
                let tried = store
                    .recent_staging_requests(&sandbox.id, env, 200)
                    .await?
                    .iter()
                    .any(|r| {
                        r.revision == revision
                            && !matches!(
                                r.state,
                                StagingState::Superseded | StagingState::Inherited
                            )
                    });
                if tried {
                    return Ok(None);
                }
                Some(note)
            }
            Asking::Person => None,
        };
        if let Some(pending) = own.iter().rev().find(|r| r.state.is_pending()) {
            return Ok(Some(pending.clone()));
        }
        let contained = self.staging_contained(sandbox, target, revision).await?;
        if contained.is_some()
            && let Some(done) = own.iter().rev().find(|r| r.state.is_success())
        {
            return Ok(Some(done.clone()));
        }
        let now = self.now();
        let state = if contained.is_some() {
            StagingState::Included
        } else {
            StagingState::Testing
        };
        let record = StagingRequestRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: sandbox.id.clone(),
            environment: env.to_owned(),
            task_id: None,
            revision: revision.to_owned(),
            state,
            ci_run_id: None,
            cd_run_id: contained.clone(),
            attempts: 0,
            note: contained
                .as_ref()
                .map(|run| format!("already on staging: run {run} deployed and verified a revision containing it"))
                .or(note),
            requested_by: requested_by.to_owned(),
            // Nobody's session hears a request of no task.
            told: true,
            created_at: now,
            updated_at: now,
        };
        if !store.insert_staging_request(&record).await? {
            return Err(AppError::Conflict(
                "the staging request could not be recorded".into(),
            ));
        }
        self.dlv_audit(
            requested_by,
            "staging.requested",
            &format!("{}/{env}", sandbox.slug),
            &format!("revision {} ({})", short(revision), state.as_str()),
        )
        .await;
        self.staging_kick(sandbox, env);
        Ok(Some(record))
    }

    /// Carries a red request of no task to the integration head once the head moved past it
    /// (SPEC-650 PST-002): one new request for the head, by the same requester, unless the head
    /// has a request (a task's too) in any state but superseded or inherited. `true` when one was
    /// recorded.
    async fn staging_carry_forward(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        target: &CdTarget,
        (head, current): (&str, Option<u64>),
        pending: &[StagingRequestRecord],
        positions: &[(String, Option<u64>)],
    ) -> AppResult<bool> {
        if !pending
            .iter()
            .any(|r| is_asked(r) && r.state == StagingState::Inherited)
        {
            return Ok(false);
        }
        let Some(at) = self
            .deps
            .git
            .first_parent_position(&self.cd_mirror(&sandbox.id), head, head)
            .await?
        else {
            return Ok(false);
        };
        let facts: Vec<Facts> = pending.iter().map(|r| facts_of(r, positions)).collect();
        let Some(waiting) = stg::follow_up(&facts, at, current) else {
            return Ok(false);
        };
        let Some(from) = pending.iter().find(|r| r.id == waiting.id) else {
            return Ok(false);
        };
        let note = format!(
            "asked again for the newer head: the full gate failed on {}",
            short(&from.revision)
        );
        Ok(self
            .staging_record_task_less(
                sandbox,
                target,
                head,
                &from.requested_by,
                Asking::Yulius(note),
            )
            .await?
            .is_some())
    }

    /// The schedule's request for the integration head (SPEC-650 PST-006): when the project's
    /// merge-first target is behind the verified head, nothing is on its way, and the merges or
    /// minutes of its `serve_after_merges` or `serve_every_minutes` were reached. It deploys as the
    /// project's creator (`staging_person`). `Ok(None)` when nothing was recorded.
    ///
    /// # Errors
    /// GitHub, Git and store failures (tried again at a later tick).
    pub async fn staging_schedule(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
    ) -> AppResult<Option<StagingRequestRecord>> {
        Ok(self.staging_schedule_checked_once(sandbox).await?.1)
    }

    /// [`App::staging_schedule`], and whether the project has a merge-first target at all.
    async fn staging_schedule_checked_once(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
    ) -> AppResult<(bool, Option<StagingRequestRecord>)> {
        if sandbox.is_playground()
            || !self.deps.git.has_cd_mirror()
            || !self.config.ci.enabled
            || self.ci_gate(sandbox).await.gate.is_none()
        {
            return Ok((false, None));
        }
        crate::ports::github::for_purpose(crate::ports::github::GitHubPurpose::Staging, async {
            let (head, target) = self.staging_target(sandbox).await?;
            let Some(target) = target else {
                return Ok((false, None));
            };
            let lock = queue_lock(&sandbox.id);
            let _queue = lock.lock().await;
            let store = &self.deps.store;
            let env = target.name.as_str();
            // An inherited request waits for another's deploy: it is not on its way (review
            // round 1 of PR 2).
            let pending = store
                .pending_staging_requests(&sandbox.id, env)
                .await?
                .iter()
                .any(|r| r.state != StagingState::Inherited);
            let mirror = self.cd_mirror(&sandbox.id);
            let Some(head_at) = self
                .deps
                .git
                .first_parent_position(&mirror, &head, &head)
                .await?
            else {
                return Ok((true, None));
            };
            let (current, _) = self.staging_current(sandbox, &target, &head).await?;
            let last_request = store
                .recent_staging_requests(&sandbox.id, env, 1)
                .await?
                .first()
                .map(|r| r.created_at.0);
            let mut last_run = None;
            for kind in [CdRunKind::Deploy, CdRunKind::Rollback, CdRunKind::Drill] {
                let run = store
                    .last_cd_run_of_kind(
                        &sandbox.id,
                        env,
                        CdStage::Dev,
                        kind,
                        crate::ports::store_ext::CdRunHistory::Latest,
                    )
                    .await?;
                last_run = last_run.max(run.map(|run| run.started_at.0));
            }
            let idle_ms = match last_request.max(last_run) {
                Some(at) => u64::try_from(self.now().0.saturating_sub(at)).unwrap_or(0),
                None => u64::MAX,
            };
            let behind = stg::Behind {
                merges_ahead: current.map(|at| head_at.saturating_sub(at)),
                idle_ms,
                pending,
            };
            let schedule = stg::Schedule {
                every_minutes: target.serve_every_minutes,
                after_merges: target.serve_after_merges,
            };
            let Some(trigger) = stg::due(schedule, behind) else {
                return Ok((true, None));
            };
            let asked = self
                .staging_record_task_less(
                    sandbox,
                    &target,
                    &head,
                    SCHEDULE_REQUESTER,
                    Asking::Yulius(trigger.note()),
                )
                .await?;
            Ok((true, asked))
        })
        .await
    }

    /// Checks each project's staging schedule at most every 5 minutes, and a project whose last
    /// check found no merge-first target once an hour (each check reads the verified head from
    /// GitHub).
    async fn staging_schedules(self: &Arc<Self>, projects: &[SandboxRecord]) {
        const EVERY_MS: i64 = 5 * 60 * 1000;
        const NO_TARGET_MS: i64 = 60 * 60 * 1000;
        let now = self.now().0;
        for project in projects {
            if project.is_playground() || project.removed_at.is_some() {
                continue;
            }
            {
                let mut checked = self
                    .staging_schedule_checked
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if checked.get(&project.id).is_some_and(|(at, target)| {
                    now.saturating_sub(*at) < if *target { EVERY_MS } else { NO_TARGET_MS }
                }) {
                    continue;
                }
                checked.insert(project.id.clone(), (now, true));
            }
            match self.staging_schedule_checked_once(project).await {
                Ok((true, _)) => {}
                Ok((false, _)) => {
                    self.staging_schedule_checked
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(project.id.clone(), (now, false));
                }
                Err(err) => {
                    tracing::warn!(sandbox = %project.slug, error = %err, "the staging schedule could not be checked");
                }
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

    /// Advances every queue with a request on its way (the background tick); a round still
    /// running when the next tick comes is not doubled (review round 2: an outcome typed twice).
    pub async fn staging_tick(self: &Arc<Self>) {
        use std::sync::atomic::{AtomicBool, Ordering};
        static RUNNING: AtomicBool = AtomicBool::new(false);
        /// Lets the next tick run again however this one ends.
        struct Done;
        impl Drop for Done {
            fn drop(&mut self) {
                RUNNING.store(false, Ordering::SeqCst);
            }
        }
        if RUNNING.swap(true, Ordering::SeqCst) {
            return;
        }
        let _done = Done;
        self.staging_tick_once().await;
    }

    async fn staging_tick_once(self: &Arc<Self>) {
        // A failed deploy's queue is visited for an hour, which restores staging (MFS-007).
        let since = yulius_domain::time::Timestamp(self.now().0 - 60 * 60 * 1000);
        let queues = match self.deps.store.staging_queues(since).await {
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
        self.tell_staging_outcomes().await;
        if let Ok(projects) = self.deps.store.sandboxes().await {
            for project in &projects {
                if !project.is_playground()
                    && project.removed_at.is_none()
                    && let Err(err) = self.tell_gate_repairs(project).await
                {
                    tracing::warn!(sandbox = %project.slug, error = %err, "integration repair notices deferred");
                }
            }
            // Staging moves on by itself, whoever finishes what (SPEC-650 PST-006); after the
            // repair notices, which a slow GitHub must not hold.
            self.staging_schedules(&projects).await;
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
        crate::ports::github::for_purpose(
            crate::ports::github::GitHubPurpose::Staging,
            self.staging_advance_inner(sandbox, env),
        )
        .await
    }

    async fn staging_advance_inner(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        env: &str,
    ) -> AppResult<()> {
        let lock = queue_lock(&sandbox.id);
        let _queue = lock.lock().await;
        let store = &self.deps.store;
        let pending = store.pending_staging_requests(&sandbox.id, env).await?;
        let (head, target) = self.staging_target(sandbox).await?;
        // The running deploy settles first, whatever the environment became (review round 1).
        let now_pending: Vec<&StagingRequestRecord> = pending.iter().collect();
        // A verified revision may be what an automatic promotion waited for (MFS-009).
        for request in self.staging_settle_deploy(&now_pending).await? {
            match self.staging_person(&request).await {
                Ok(Some(owner)) => self.staging_auto_promote(sandbox, &owner).await,
                Ok(None) => {
                    tracing::warn!(sandbox = %sandbox.slug, request = %request.id, "no active owner with write access runs the promotion waiting for staging")
                }
                Err(err) => {
                    tracing::warn!(error = %err, sandbox = %sandbox.slug, request = %request.id, "the promotion waiting for staging could not run")
                }
            }
        }
        let pending = store.pending_staging_requests(&sandbox.id, env).await?;
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
        let Some(target) = target else {
            return Ok(());
        };
        self.staging_restore(sandbox, &target).await?;
        let positions = self
            .staging_positions(sandbox, &head, pending.iter().map(|r| r.revision.as_str()))
            .await?;
        self.staging_judge(sandbox, &head, &positions, &pending)
            .await?;
        let pending = store.pending_staging_requests(&sandbox.id, env).await?;
        // A Git failure ends the round, to be tried again: it never reads as "not on the branch".

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
        let (current, served) = self.staging_current(sandbox, &target, &head).await?;
        let (pending, positions) = if self
            .staging_carry_forward(sandbox, &target, (&head, current), &pending, &positions)
            .await?
        {
            let pending = store.pending_staging_requests(&sandbox.id, env).await?;
            let positions = self
                .staging_positions(sandbox, &head, pending.iter().map(|r| r.revision.as_str()))
                .await?;
            (pending, positions)
        } else {
            (pending, positions)
        };
        let facts: Vec<Facts> = pending.iter().map(|r| facts_of(r, &positions)).collect();
        let pick = stg::pick(&facts, current);
        for id in &pick.included {
            if let Some(request) = pending.iter().find(|r| &r.id == id) {
                let change = StagingChange {
                    cd_run_id: served.as_ref().map(|(_, run)| run.clone()),
                    note: served.as_ref().map(|(sha, _)| {
                        let earlier = if request.state == StagingState::Inherited {
                            request.note.as_deref().unwrap_or("")
                        } else {
                            ""
                        };
                        format!(
                            "included in verified staging revision {}. {earlier}",
                            short(sha)
                        )
                    }),
                    ..StagingChange::default()
                };
                self.staging_move(request, StagingState::Included, change)
                    .await?;
            }
        }
        let Some(deploy) = pick
            .deploy
            .as_ref()
            .and_then(|id| pending.into_iter().find(|r| &r.id == id))
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
        head: &str,
        positions: &[(String, Option<u64>)],
        pending: &[StagingRequestRecord],
    ) -> AppResult<()> {
        let integration = delivery_profile(sandbox).integration_branch;
        // A person's or the schedule's red request is green again once a rerun of its revision
        // passed the full gate (review round 1, finding 6).
        for request in pending
            .iter()
            .filter(|r| is_asked(r) && r.state == StagingState::Inherited)
            .take(MAX_PENDING)
        {
            let verdict = self
                .deps
                .store
                .integration_verdicts(
                    &sandbox.id,
                    &integration,
                    std::slice::from_ref(&request.revision),
                    super::GateRead::Latest,
                )
                .await?
                .into_iter()
                .next();
            if let Some(run) = verdict
                && run.state == RunState::Passed
                && !run.jobs.iter().any(|j| j.skipped)
                && request.ci_run_id.as_deref() != Some(run.run.as_str())
            {
                let change = StagingChange {
                    ci_run_id: Some(run.run.clone()),
                    note: Some(format!("the full gate passed on a rerun: run {}", run.run)),
                    ..StagingChange::default()
                };
                self.staging_move(request, StagingState::Ready, change)
                    .await?;
            }
        }
        // The newest waiting revision is judged first, and a request a judged revision above it
        // answers needs no run of its own (SPEC-653).
        let mut testing: Vec<(u64, &StagingRequestRecord)> = pending
            .iter()
            .filter(|r| r.state == StagingState::Testing)
            .filter_map(|r| Some((positions.iter().find(|(sha, _)| sha == &r.revision)?.1?, r)))
            .collect();
        testing.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        testing.truncate(MAX_PENDING);
        let Some(lowest) = testing.iter().map(|(at, _)| *at).min() else {
            return Ok(());
        };
        let mut window = self.staging_window(sandbox, head, lowest).await?;
        for (at, request) in &testing {
            if !window.iter().any(|(sha, _)| sha == &request.revision) {
                window.push((request.revision.clone(), *at));
            }
        }
        let revisions: Vec<String> = window.iter().map(|(sha, _)| sha.clone()).collect();
        let verdicts = self
            .deps
            .store
            .integration_verdicts(
                &sandbox.id,
                &integration,
                &revisions,
                super::GateRead::Latest,
            )
            .await?;
        let verdict = |sha: &str| verdicts.iter().find(|v| v.sha == sha);
        let points: Vec<judge::Point> = window
            .iter()
            .map(|(sha, position)| judge::Point {
                position: *position,
                gate: verdict(sha).and_then(super::GateVerdict::gate),
                asked: testing.iter().any(|(_, r)| &r.revision == sha),
            })
            .collect();
        let plan = judge::judging(&points);
        let named = |position: u64| {
            let sha = window
                .iter()
                .find(|(_, at)| *at == position)
                .map(|(sha, _)| sha.as_str());
            let run = sha.and_then(verdict).map(|v| v.run.as_str());
            (short(sha.unwrap_or("?")), run.unwrap_or("?"))
        };
        for (at, request) in &testing {
            let answered = plan.covered.iter().map(|(p, green)| (p, green, true));
            let answered = answered.chain(plan.inherited.iter().map(|(p, red)| (p, red, false)));
            if let Some((_, above, green)) = answered.into_iter().find(|(p, _, _)| *p == at) {
                let (revision, run) = named(*above);
                let verdict = if green {
                    "passed on"
                } else {
                    "failed only on failures from earlier merges on"
                };
                let note = format!(
                    "the full gate {verdict} revision {revision}, which contains it: run {run}; it needs no run of its own, and staging deploys it with a verified revision containing it"
                );
                let change = StagingChange {
                    note: Some(note),
                    ..StagingChange::default()
                };
                self.staging_move(request, StagingState::Inherited, change)
                    .await?;
            } else if !plan.unresolved.contains(at)
                && let Some(run) = verdict(&request.revision)
            {
                self.staging_verdict(sandbox, request, run).await?;
            }
        }
        let next = plan.run.and_then(|at| {
            testing
                .iter()
                .find(|(p, r)| *p == at && verdict(&r.revision).is_none())
        });
        let Some((_, request)) = next else {
            return Ok(());
        };
        let gate = self
            .ci_gate(sandbox)
            .await
            .gate
            .filter(|_| self.config.ci.enabled);
        let Some(gate) = gate else {
            // Nothing can judge them, and they would hold every newer request back.
            for (_, request) in testing
                .iter()
                .filter(|(_, r)| verdict(&r.revision).is_none())
            {
                let change = StagingChange {
                    note: Some("no full gate can run: this project has no CI gate in force".into()),
                    ..StagingChange::default()
                };
                self.staging_move(request, StagingState::GateFailed, change)
                    .await?;
            }
            return Ok(());
        };
        // CI inventory holds the same scheduling lock while it takes its snapshot and records
        // branch runs. A concurrent inventory cannot double-admit this SHA. One full-gate run at a
        // time (SPEC-653 NWF-005).
        let scheduler = self.ci_sync_lock(&sandbox.id);
        let guard = scheduler.schedule.lock().await;
        if !self
            .deps
            .store
            .integration_verdicts(
                &sandbox.id,
                &integration,
                std::slice::from_ref(&request.revision),
                super::GateRead::Latest,
            )
            .await?
            .is_empty()
            || self.deps.store.active_ci_runs().await?.iter().any(|r| {
                r.sandbox_id == sandbox.id
                    && (matches!(r.reference, CiRef::Candidate(_))
                        || matches!(&r.reference, CiRef::Branch(branch) if branch == &integration))
            })
        {
            return Ok(());
        }
        let run = self
            .ci_record_placed(
                sandbox,
                CiRef::Candidate(request.revision.clone()),
                &request.revision,
                &gate,
                "yulius",
                self.now(),
            )
            .await?;
        drop(guard);
        self.ci_publish(&run.id).await;
        let change = StagingChange {
            ci_run_id: Some(run.id.clone()),
            note: Some(format!("the full gate runs on it: run {}", run.id)),
            ..StagingChange::default()
        };
        self.staging_move(request, StagingState::Testing, change)
            .await?;
        self.ci_wake();
        Ok(())
    }

    /// The first-parent revisions of the verified head from the head down to position `lowest`,
    /// with their positions, at most [`WINDOW`] (SPEC-653 NWF-007).
    ///
    /// # Errors
    /// Git failures: never read as "not on the branch".
    async fn staging_window(
        &self,
        sandbox: &SandboxRecord,
        head: &str,
        lowest: u64,
    ) -> AppResult<Vec<(String, u64)>> {
        let mirror = self.cd_mirror(&sandbox.id);
        let git = &self.deps.git;
        let Some(top) = git.first_parent_position(&mirror, head, head).await? else {
            return Ok(Vec::new());
        };
        let mut window = vec![(head.to_owned(), top)];
        while let Some((oldest, at)) = window.last().cloned()
            && at > lowest
            && window.len() < WINDOW
        {
            let batch = git.preceding_commits(&mirror, &oldest).await?;
            if batch.is_empty() {
                break;
            }
            for (sha, position) in batch.into_iter().zip((lowest..at).rev()) {
                window.push((sha, position));
            }
            window.truncate(WINDOW);
        }
        Ok(window)
    }

    /// Moves a testing request on the full gate's verdict at its own revision (SPEC-575
    /// MFS-003, MFS-007; SPEC-612; SPEC-650 PST-002); nothing while the verdict or the failures'
    /// attribution is still to come.
    async fn staging_verdict(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        request: &StagingRequestRecord,
        run: &super::GateVerdict,
    ) -> AppResult<()> {
        let verdict = if run.state == RunState::Cancelled {
            RunState::Error
        } else {
            run.state
        };
        let Some(mut next) = stg::after_verdict(Some(verdict)) else {
            return Ok(());
        };
        let mut note = format!("the full gate passed: run {}", run.run);
        if next == StagingState::GateFailed || run.jobs.iter().any(|j| j.skipped) {
            let history = self
                .gate_failures_at(
                    sandbox,
                    &self.cd_mirror(&sandbox.id),
                    &request.revision,
                    false,
                )
                .await?;
            if history.waiting {
                return Ok(());
            }
            let requester = match &request.task_id {
                Some(id) => self.deps.store.task(id).await?.map(|t| t.number),
                None => run.owner,
            };
            if let Some((inherited, detail)) = history.describe(run, requester) {
                next = stg::after_failed_gate(false, inherited);
                let outcome = if run.state == RunState::Passed {
                    "skipped an unresolved job"
                } else {
                    "failed"
                };
                note = format!(
                    "the full gate {outcome} on revision {}: run {} ({detail}); staging did not change",
                    short(&request.revision),
                    run.run
                );
            }
        }
        // A person's or the schedule's request waits for the next green revision (SPEC-650
        // PST-002).
        if is_asked(request) && next != StagingState::Ready {
            next = stg::after_failed_gate(true, next == StagingState::Inherited);
            note = format!(
                "waiting for green tests: the full gate failed on {} (run {}); the next merged revision is tested and deployed once green",
                short(&request.revision),
                run.run
            );
        }
        let change = StagingChange {
            ci_run_id: Some(run.run.clone()),
            note: Some(note),
            ..StagingChange::default()
        };
        self.staging_move(request, next, change).await?;
        Ok(())
    }

    /// Reads the outcome of the deploy running for the queue (SPEC-575 MFS-007, MFS-008): a
    /// passed run verifies its request, a failed one fails it, an errored or lost one makes it
    /// ready again until its attempts run out.
    async fn staging_settle_deploy(
        self: &Arc<Self>,
        pending: &[&StagingRequestRecord],
    ) -> AppResult<Vec<StagingRequestRecord>> {
        let mut verified = Vec::new();
        for request in pending
            .iter()
            .filter(|r| r.state == StagingState::Deploying)
        {
            let run = match &request.cd_run_id {
                Some(id) => self.deps.store.cd_run(id, true).await?,
                None => None,
            };
            let end = match run.as_ref().map(|r| r.state) {
                Some(RunState::Running | RunState::Queued) => continue,
                Some(RunState::Passed) => DeployEnd::Passed,
                Some(RunState::Failed) => DeployEnd::Failed,
                // A person stopped it: their decision stands (review round 1).
                Some(RunState::Cancelled) => DeployEnd::Cancelled,
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
                (DeployEnd::Failed, _) => {
                    let barriers = run
                        .as_ref()
                        .map(|r| yulius_domain::rollback::failed_barriers(&r.output))
                        .unwrap_or_default();
                    match barriers.first() {
                        // Staging is not taken back past what the deploy said is irreversible.
                        Some((_, reason)) => format!(
                            "the deploy or verify failed: run {run_id}; it announced a rollback barrier ({reason}), so staging stays where it left it, and a person decides"
                        ),
                        None => format!(
                            "the deploy or verify failed: run {run_id}; Yulius restores staging's last verified release when it keeps one"
                        ),
                    }
                }
                (DeployEnd::Cancelled, _) => {
                    format!("a person cancelled its deploy, run {run_id}; ask again to deploy it")
                }
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
            if self.staging_move(request, next, change).await? && next == StagingState::Verified {
                verified.push((*request).clone());
            }
        }
        Ok(verified)
    }

    /// Starts the deploy of `request`: it moves to deploying first (one per environment, by the
    /// store's rule), then its run starts and is recorded on it.
    async fn staging_start_deploy(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        env: &str,
        mut request: StagingRequestRecord,
    ) -> AppResult<()> {
        // Another run of the environment (a rollback, its drill) runs, or an update holds new
        // deploys: the deploy waits, costing no attempt (review round 1).
        if self.staging_must_wait(sandbox, env).await? {
            return Ok(());
        }
        let moved = self
            .staging_move(
                &request,
                StagingState::Deploying,
                StagingChange {
                    note: Some(format!("deploying revision {}", short(&request.revision))),
                    clear_cd_run: true,
                    ..StagingChange::default()
                },
            )
            .await?;
        if !moved {
            return Ok(());
        }
        request.state = StagingState::Deploying;
        // The run is the task's (SPEC-631 SWK-004); its owner's access deploys it. A request of
        // no task deploys as the person who asked, or for the schedule (SPEC-650 PST-003).
        let task = match &request.task_id {
            Some(task) => self.deps.store.task(task).await?,
            None => None,
        };
        let person = self.staging_person(&request).await?;
        let started = match &person {
            Some(person) => {
                let actor = match &task {
                    Some(task) => CdActor::ForTask(task),
                    None => CdActor::Person(person),
                };
                let note = format!(
                    "{DEPLOY_NOTE} {} ({})",
                    request.id,
                    request
                        .task_id
                        .as_ref()
                        .map_or("a delivery", |_| "a task's delivery")
                );
                self.cd_start(
                    sandbox,
                    env,
                    Requester::Person {
                        administrator: false,
                        confirmation: None,
                    },
                    &actor,
                    CdRequest::DeployRevision {
                        revision: &request.revision,
                        note: &note,
                    },
                )
                .await
            }
            None => Err(AppError::Forbidden(if request.task_id.is_some() {
                "its task's owner is no longer active or lost write access: nobody can deploy for it".into()
            } else {
                "the person it deploys for is no longer active or lost write access: nobody can deploy for it".into()
            })),
        };
        match started {
            Ok(run) => {
                self.staging_move(
                    &request,
                    StagingState::Deploying,
                    StagingChange {
                        cd_run_id: Some(run.id.clone()),
                        note: Some(format!(
                            "deploying revision {}: run {}",
                            short(&request.revision),
                            run.id
                        )),
                        ..StagingChange::default()
                    },
                )
                .await?;
            }
            // What started meanwhile (a rollback between the look and the start, an update
            // closing the gate) is waited for, costing no attempt.
            Err(AppError::Conflict(_)) if self.staging_must_wait(sandbox, env).await? => {
                self.staging_move(
                    &request,
                    StagingState::Ready,
                    StagingChange {
                        note: Some(
                            "waiting for another run of the environment or an update".into(),
                        ),
                        ..StagingChange::default()
                    },
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
                self.staging_move(
                    &request,
                    next,
                    StagingChange {
                        attempts: Some(attempts),
                        note: Some(format!(
                            "the deploy could not start: {}",
                            err.public_message()
                        )),
                        ..StagingChange::default()
                    },
                )
                .await?;
            }
        }
        Ok(())
    }

    /// What staging serves now: the commit and run of the current release of its first part
    /// in the kept history (deploys, restores and rollbacks alike), else the newest passed
    /// deploy's (SPEC-575 MFS-006; review round 1: a rollback moves it back).
    async fn staging_serving(
        &self,
        sandbox: &SandboxRecord,
        target: &CdTarget,
    ) -> AppResult<Option<(String, String)>> {
        let store = &self.deps.store;
        let part = target.part_names().into_iter().next().unwrap_or_default();
        let records: Vec<_> = store
            .cd_releases(&sandbox.id, &target.name, CdStage::Dev)
            .await?
            .into_iter()
            .filter(|r| r.part == part)
            .collect();
        if let Some(current) = yulius_domain::rollback::History(&records).current() {
            return Ok(Some((current.sha.clone(), current.run_id.clone())));
        }
        Ok(store
            .last_passed_cd_run(&sandbox.id, &target.name, CdStage::Dev)
            .await?
            .map(|run| (run.sha, run.id)))
    }

    /// The position of what staging serves and the run that put it there.
    async fn staging_current(
        &self,
        sandbox: &SandboxRecord,
        target: &CdTarget,
        head: &str,
    ) -> AppResult<(Option<u64>, Option<(String, String)>)> {
        let Some((sha, run)) = self.staging_serving(sandbox, target).await? else {
            return Ok((None, None));
        };
        // A served commit the mirror no longer has (a force push dropped it) is off the branch,
        // never an error that would stop every round (review round 2).
        let at = self
            .deps
            .git
            .first_parent_position(&self.cd_mirror(&sandbox.id), head, &sha)
            .await
            .ok()
            .flatten();
        Ok((at, Some((sha, run))))
    }

    /// The run that put on staging a revision containing `revision`, if what staging serves
    /// does (SPEC-575 MFS-005: a delivery already on staging, also from before merge-first, is
    /// not deployed twice).
    async fn staging_contained(
        &self,
        sandbox: &SandboxRecord,
        target: &CdTarget,
        revision: &str,
    ) -> AppResult<Option<String>> {
        let Some((serving, run)) = self.staging_serving(sandbox, target).await? else {
            return Ok(None);
        };
        let contains = self
            .deps
            .git
            .is_ancestor(&self.cd_mirror(&sandbox.id), revision, &serving)
            .await
            .unwrap_or(false);
        Ok(contains.then_some(run))
    }

    /// The integration head a promotion must promote, read through the CD mirror (SPEC-575
    /// MFS-009): `Ok(None)` without a merge-first target (any head); with one, the verified head
    /// when staging serves it, verified, else `Err` with why it waits for staging. A head or table
    /// that cannot be read is the error.
    ///
    /// # Errors
    /// `Conflict` when the verified head or its tables cannot be read.
    pub(crate) async fn staging_promotion_head(
        &self,
        sandbox: &SandboxRecord,
    ) -> AppResult<Result<Option<String>, String>> {
        // A playground has no GitHub and no CD (SPEC-099), and a host whose Git keeps no CD
        // mirror runs no CD: no staging to wait for.
        if sandbox.is_playground() || !self.deps.git.has_cd_mirror() {
            return Ok(Ok(None));
        }
        // The verified tables decide, and a promotion whose tables cannot be read is refused,
        // whatever the project: an agent breaking its clone can only cause a refusal, never a
        // bypass (review round 1 of slice 4).
        let (head, target) = self.staging_target(sandbox).await.map_err(|err| {
            AppError::Conflict(format!(
                "nothing was promoted: the verified integration head and its CD tables cannot be read now ({}); promote again once they read (a fetch problem passes by itself, an invalid yulius.toml needs a fix)",
                err.public_message()
            ))
        })?;
        let Some(target) = target else {
            return Ok(Ok(None));
        };
        let verified = self.staging_verified_revision(sandbox, &target).await?;
        if verified.as_deref() == Some(head.as_str()) {
            return Ok(Ok(Some(head)));
        }
        Ok(Err(format!(
            "nothing was promoted: production is promoted from the revision [cd.{}] verified, and the integration head {} is {}; promote once staging verified it",
            target.name,
            short(&head),
            match &verified {
                Some(sha) => format!("not it (staging verified {})", short(sha)),
                None => "not verified there (staging verified nothing yet)".to_owned(),
            }
        )))
    }

    /// What `delivery_status` tells `task` about its staging (SPEC-575 MFS-013), once its work
    /// merged into a project with a merge-first target: that its request is due, or where its
    /// newest request stands. `None` without a landing or a merge-first target, and when the
    /// verified tables cannot be read now (`delivery_status` still answers).
    pub(crate) async fn staging_note(
        &self,
        task: &crate::model::TaskRecord,
        sandbox: &SandboxRecord,
        receipt_pr: Option<u64>,
    ) -> Option<String> {
        if sandbox.is_playground() || !self.deps.git.has_cd_mirror() {
            return None;
        }
        let branch = delivery_profile(sandbox).integration_branch;
        let store = &self.deps.store;
        let read = async {
            let landing = store.staging_landing(&task.id, &branch, receipt_pr).await?;
            if landing.is_none() && receipt_pr.is_none() {
                return Ok(None);
            }
            let displaying = crate::ports::github::current_purpose()
                == crate::ports::github::GitHubPurpose::Display;
            // A receipt describes this landing. Its verified config stays readable locally.
            // The actual deploy independently authorizes the current integration head.
            let local = if displaying && let Some(sha) = &landing {
                self.cd_verified_config(sandbox, sha).await.ok()
            } else {
                None
            };
            let target = if let Some(config) = local {
                config.merge_first().cloned()
            } else if displaying && self.github_budget().await?.display_paused {
                return Err(AppError::Capacity(
                    "Live staging status refresh is paused to reserve GitHub requests for delivery.".into(),
                ));
            } else {
                self.staging_target(sandbox).await?.1
            };
            let Some(target) = target else {
                return Ok(None);
            };
            let requests = match &landing {
                Some(sha) => {
                    store
                        .staging_requests_of(&sandbox.id, &target.name, Some(&task.id), sha)
                        .await?
                }
                None => Vec::new(),
            };
            AppResult::Ok(Some((landing, target, requests)))
        };
        let (landing, target, requests) = match read.await {
            Ok(found) => found?,
            Err(AppError::Capacity(note)) => return Some(note),
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "delivery_status: staging could not be read");
                return None;
            }
        };
        let lead = format!("Staging ([cd.{}], merge-first)", target.name);
        let Some(landing) = landing else {
            return Some(format!(
                "{lead}: the landing of pull request #{} on {branch} awaits confirmation. Read delivery_status to reconcile the merged revision before requesting staging.",
                receipt_pr?
            ));
        };
        // The receipt and cd_deploy make the same decision about this task's older requests.
        // Containment only changes a new request's initial state, which the call will report.
        let facts = requests
            .iter()
            .map(|r| facts_of(r, &[]))
            .collect::<Vec<_>>();
        let request = match stg::admit(&landing, &facts, false) {
            Admission::Existing(id) | Admission::Retry(id) => {
                requests.iter().find(|r| r.id == id)?
            }
            Admission::Refused(reason) => {
                return Some(format!(
                    "{lead}: your merged revision {}. {reason}.",
                    short(&landing)
                ));
            }
            Admission::New { .. } => {
                return Some(format!(
                    "{lead}: your merged revision {} has no staging request. Once your whole \
                 delivery has landed, ask once with cd_deploy {} before ending your turn. Yulius \
                 runs the full suite, then deploys and verifies that revision. If your final step \
                 requires the outcome, leave it in progress. A running task with work left receives the outcome.",
                    short(&landing),
                    serde_json::json!({"environment": target.name}),
                ));
            }
        };
        let next = staging_next(request);
        Some(format!(
            "{lead}: request {} for revision {} is {}{}{}.{}",
            request.id,
            short(&request.revision),
            request.state.as_str(),
            run_ids(request),
            request
                .note
                .as_deref()
                .map_or_else(String::new, |n| format!(" ({n})")),
            if next.is_empty() {
                String::new()
            } else {
                format!(" {next}")
            },
        ))
    }

    /// The revision production may be promoted from (SPEC-575 MFS-009): what staging serves,
    /// when a verified request deployed it.
    pub(crate) async fn staging_verified_revision(
        &self,
        sandbox: &SandboxRecord,
        target: &CdTarget,
    ) -> AppResult<Option<String>> {
        let Some((serving, _)) = self.staging_serving(sandbox, target).await? else {
            return Ok(None);
        };
        let verified = self
            .deps
            .store
            .staging_verified(&sandbox.id, &target.name, &serving)
            .await?;
        Ok(verified.then_some(serving))
    }

    /// Each revision's first-parent position on the verified integration head (in the CD
    /// mirror); `None` for one off that history.
    ///
    /// # Errors
    /// Git failures: never read as "not on the branch".
    async fn staging_positions<'a>(
        &self,
        sandbox: &SandboxRecord,
        head: &str,
        revisions: impl Iterator<Item = &'a str>,
    ) -> AppResult<Vec<(String, Option<u64>)>> {
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
                .await?;
            out.push((revision.to_owned(), at));
        }
        Ok(out)
    }

    /// Whether a deploy of `env` must wait now: another run of it goes on, or an update holds
    /// new deploys.
    async fn staging_must_wait(&self, sandbox: &SandboxRecord, env: &str) -> AppResult<bool> {
        Ok(self.deploy_permit().is_err()
            || self
                .deps
                .store
                .running_cd_runs()
                .await?
                .iter()
                .any(|run| run.sandbox_id == sandbox.id && run.environment == env))
    }

    /// The task's owner when active with write access to its sandbox, as every deploy for a
    /// task needs (review round 1, B1).
    async fn staging_owner(
        &self,
        task: &crate::model::TaskRecord,
    ) -> AppResult<Option<UserRecord>> {
        self.staging_writer(&task.sandbox_id, &task.owner).await
    }

    /// `user` when active with write access to `sandbox`, now.
    async fn staging_writer(
        &self,
        sandbox: &yulius_domain::ids::SandboxId,
        user: &yulius_domain::ids::UserId,
    ) -> AppResult<Option<UserRecord>> {
        let Some(user) = self
            .deps
            .store
            .user(user)
            .await?
            .filter(|u| u.disabled_at.is_none())
        else {
            return Ok(None);
        };
        match self.require(&user, sandbox, Action::Work).await {
            Ok(_) => Ok(Some(user)),
            Err(AppError::Forbidden(_) | AppError::NotFound) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// The person a request's deploy acts for, with write access now (SPEC-650 PST-003): its
    /// task's owner; the person who asked; for the schedule, the project's creator, else the
    /// newest requester of the environment's queue (a person, or a task's owner).
    async fn staging_person(
        &self,
        request: &StagingRequestRecord,
    ) -> AppResult<Option<UserRecord>> {
        if let Some(found) = self.staging_asker(request).await? {
            return Ok(Some(found));
        }
        if request.requested_by != SCHEDULE_REQUESTER {
            return Ok(None);
        }
        let store = &self.deps.store;
        if let Some(sandbox) = store.sandbox(&request.sandbox_id).await?
            && let Some(creator) = self
                .staging_writer(&request.sandbox_id, &sandbox.created_by)
                .await?
        {
            return Ok(Some(creator));
        }
        for other in store
            .recent_staging_requests(&request.sandbox_id, &request.environment, 50)
            .await?
        {
            if other.id != request.id
                && let Some(found) = self.staging_asker(&other).await?
            {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    /// Who asked for `request` with write access now: its task's owner or the person.
    async fn staging_asker(&self, request: &StagingRequestRecord) -> AppResult<Option<UserRecord>> {
        if let Some(task) = &request.task_id {
            return match self.deps.store.task(task).await? {
                Some(task) => self.staging_owner(&task).await,
                None => Ok(None),
            };
        }
        let Some(id) = request
            .requested_by
            .strip_prefix(PERSON_REQUESTER)
            .and_then(|id| yulius_domain::ids::UserId::parse(id).ok())
        else {
            return Ok(None);
        };
        self.staging_writer(&request.sandbox_id, &id).await
    }

    /// After the queue's newest deploy failed, puts staging back on its last verified release
    /// (SPEC-575 MFS-007): each part's current kept release is redeployed and verified once, as a
    /// rollback run, one part per round. A failed deploy records no release, so the history's
    /// current release is the last verified one. Nothing happens once a later run of the
    /// environment ran, or for a part with no verified release yet.
    async fn staging_restore(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        target: &CdTarget,
    ) -> AppResult<()> {
        let store = &self.deps.store;
        let env = target.name.as_str();
        let runs: Vec<_> = store
            .cd_runs(&sandbox.id, super::cd::CD_RUN_LIST_LIMIT)
            .await?
            .into_iter()
            .filter(|run| run.environment == env)
            .collect();
        if runs.iter().any(|run| run.state == RunState::Running) {
            return Ok(());
        }
        let Some(failed_at) = runs.iter().position(|run| {
            run.kind == CdRunKind::Deploy
                && run
                    .note
                    .as_deref()
                    .is_some_and(|n| n.starts_with(DEPLOY_NOTE))
        }) else {
            return Ok(());
        };
        let failed = &runs[failed_at];
        if failed.state != RunState::Failed {
            return Ok(());
        }
        let after = &runs[..failed_at];
        // Anything else that deployed meanwhile (a person's rollback) decides what runs there.
        if after.iter().any(|run| {
            !run.note
                .as_deref()
                .is_some_and(|n| n.starts_with(RESTORE_NOTE))
        }) {
            return Ok(());
        }
        let releases = store.cd_releases(&sandbox.id, env, CdStage::Dev).await?;
        // A barrier the failed deploy announced keeps that part where it left it (RBK-022,
        // review round 2 of slice 2): a person decides.
        let barriers = store
            .cd_run(&failed.id, true)
            .await?
            .map(|run| yulius_domain::rollback::failed_barriers(&run.output))
            .unwrap_or_default();
        for part in target.part_names() {
            if barriers.iter().any(|(p, _)| p.is_empty() || *p == part) {
                continue;
            }
            let label = if part.is_empty() {
                "staging".to_owned()
            } else {
                part.clone()
            };
            let note = format!("{RESTORE_NOTE} {label} after failed run {}", failed.id);
            if after
                .iter()
                .any(|run| run.note.as_deref().is_some_and(|n| n.starts_with(&note)))
                || !releases.iter().any(|r| r.part == part)
            {
                continue;
            }
            let request = store
                .recent_staging_requests(&sandbox.id, env, 50)
                .await?
                .into_iter()
                .find(|r| r.cd_run_id.as_deref() == Some(failed.id.as_str()));
            let person = match &request {
                Some(request) => self.staging_person(request).await?,
                None => None,
            };
            let Some(person) = person else {
                return Ok(());
            };
            if let Err(err) = self
                .cd_start(
                    sandbox,
                    env,
                    Requester::Person {
                        administrator: false,
                        confirmation: None,
                    },
                    &CdActor::Person(&person),
                    CdRequest::Restore {
                        part: &part,
                        note: &note,
                    },
                )
                .await
            {
                tracing::warn!(error = %err, sandbox = %sandbox.slug, "staging could not be restored to its last verified release");
            }
            return Ok(());
        }
        Ok(())
    }

    /// Record finished tasks' outcomes silently. Running tasks with work left can also hear
    /// their own delivery verdict once, subject to the usual terminal and person holds, and so
    /// can a finished task whose idle agent still runs background work, which waits for it
    /// (SPEC-631 SWK-001): that work would keep it from Done.
    pub async fn tell_staging_outcomes(self: &Arc<Self>) -> usize {
        let Ok(untold) = self.deps.store.untold_staging_requests().await else {
            return 0;
        };
        let mut told = 0;
        for request in untold {
            let store = &self.deps.store;
            let Some(task_id) = &request.task_id else {
                continue;
            };
            // Chromium startup can hold the browser slot. Leave the outcome for another tick
            // if its handoffs cannot be checked, and read the recipient after this slow lookup.
            let browser = tokio::time::timeout(
                TASK_VIEW_BUDGET,
                self.browser_tab_requests(&request.sandbox_id),
            )
            .await;
            let task = match store.task(task_id).await {
                Ok(Some(task)) => task,
                Ok(None) => {
                    let _ = store.mark_staging_told(&request).await;
                    continue;
                }
                Err(_) => continue,
            };
            if task.status.is_resting() || task.status == TaskStatus::Parked {
                let _ = store.mark_staging_told(&request).await;
                continue;
            }
            let finished = task.status == TaskStatus::Running
                && matches!(self.accepts_machine_notice(task_id).await, Ok(false));
            // A finished task is decided once its turn is over and nothing holds it (a hold
            // its own outcome raised included): a turn or a hold can still end in a wait.
            if finished && !task.activity.is_some_and(Activity::is_idle) {
                continue;
            }
            // Its own background work waits for this outcome and keeps it from Done.
            let awaited = finished && self.agent_goes_on_by_itself(&task).await;
            if finished && !awaited {
                let _ = store.mark_staging_told(&request).await;
                continue;
            }
            if awaited {
                crate::race::reached(
                    crate::race::Point::AwaitedOutcomeDecided,
                    std::path::Path::new(task_id.as_str()),
                );
            }
            let mut text = staging_outcome_text(&request);
            if awaited {
                text.push(' ');
                text.push_str(AWAITED_OUTCOME);
            }
            let text = self.redact_for_sandbox(&request.sandbox_id, &text).await;
            if !browser.is_ok_and(|requests| !requests.contains(task_id.as_str())) {
                continue;
            }
            // Check the current recipient, their holds and the exact outcome immediately before
            // pasting into the observed live attempt.
            let still_due = || async {
                let Ok(Some(current)) = store.task(task_id).await else {
                    return false;
                };
                current.status == task.status
                    && current.activity != Some(Activity::NeedsAttention)
                    && (!awaited
                        || (matches!(self.accepts_machine_notice(task_id).await, Ok(false))
                            && self.agent_goes_on_by_itself(&current).await))
                    && !self.update_holds_starts()
                    && !self.is_paused(&current).await
                    && matches!(store.pending_hold(task_id).await, Ok(None))
                    && matches!(store.task_needs_you(task_id).await, Ok(None))
                    && matches!(store.staging_request(&request.id).await, Ok(Some(current)) if current == request)
            };
            let delivered = match task.status {
                TaskStatus::Running => {
                    let Ok(Some(attempt)) = store.live_attempt(task_id).await else {
                        continue;
                    };
                    let paste = format!(
                        "\x1b[200~{}\x1b[201~",
                        crate::task_message::clean_message(&text)
                    );
                    self.type_into_session_if(
                        &task,
                        paste.as_bytes(),
                        Typed::Notice(&attempt.id),
                        if awaited {
                            Sender::YuliusAwaited
                        } else {
                            Sender::Yulius
                        },
                        Some(still_due()),
                    )
                    .await
                    .is_ok()
                }
                _ => false,
            };
            if delivered && store.mark_staging_told(&request).await.unwrap_or(false) {
                self.publish_staging_task(&request).await;
                told += 1;
            }
        }
        told
    }

    /// Update the task view within a bounded wait. A busy summary asks the sandbox's
    /// viewers to refetch, so checkout or browser work cannot strand a staging receipt.
    pub(super) async fn publish_staging_task(&self, request: &StagingRequestRecord) {
        if let Some(task) = &request.task_id
            && tokio::time::timeout(TASK_VIEW_BUDGET, self.publish_task(task))
                .await
                .is_err()
        {
            self.publish_resync(Some(request.sandbox_id.clone()));
        }
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
            self.publish_staging_task(request).await;
            tracing::info!(request = %request.id, from = request.state.as_str(), to = to.as_str(), revision = %short(&request.revision), "staging request moved");
        }
        Ok(moved)
    }
}

/// How a request of no task is recorded.
enum Asking {
    /// A person asks now.
    Person,
    /// Yulius asks by itself, with this note: a red request carried to the head, or the
    /// schedule (SPEC-650 PST-002, PST-006).
    Yulius(String),
}

/// Whether `request` is a person's or the schedule's (SPEC-650), read from who asked: a deleted
/// task's request has no task either, and stays a task's (review round 1, finding 2).
pub(crate) fn is_asked(request: &StagingRequestRecord) -> bool {
    request.task_id.is_none()
        && (request.requested_by.starts_with(PERSON_REQUESTER)
            || request.requested_by == SCHEDULE_REQUESTER)
}

fn facts_of(request: &StagingRequestRecord, positions: &[(String, Option<u64>)]) -> Facts {
    Facts {
        id: request.id.clone(),
        // A deleted task's request is still a task's to the decisions (finding 2).
        task: match &request.task_id {
            Some(task) => Some(task.as_str().to_owned()),
            None if is_asked(request) => None,
            None => Some(request.requested_by.clone()),
        },
        revision: request.revision.clone(),
        state: request.state,
        position: positions
            .iter()
            .find(|(sha, _)| sha == &request.revision)
            .and_then(|(_, at)| *at),
        attempts: request.attempts,
    }
}

impl App {
    /// One request as the CD page and `cd_status` show it.
    async fn staging_request_view(
        &self,
        request: &StagingRequestRecord,
    ) -> AppResult<yulius_contracts::api::cd::CdStagingRequest> {
        let store = &self.deps.store;
        let task_number = match &request.task_id {
            Some(task) => store.task(task).await?.map(|t| t.number),
            None => None,
        };
        // A person's login; the schedule and a deleted task's request name nobody (finding 2).
        let asked_by = match request
            .requested_by
            .strip_prefix(PERSON_REQUESTER)
            .filter(|_| request.task_id.is_none())
            .and_then(|id| yulius_domain::ids::UserId::parse(id).ok())
        {
            Some(id) => Some(
                store
                    .user(&id)
                    .await?
                    .map_or_else(|| "a removed person".to_owned(), |u| u.label()),
            ),
            None => None,
        };
        Ok(yulius_contracts::api::cd::CdStagingRequest {
            id: request.id.clone(),
            task_number,
            asked_by,
            scheduled: request.task_id.is_none() && request.requested_by == SCHEDULE_REQUESTER,
            revision: request.revision.clone(),
            state: request.state.as_str().to_owned(),
            ci_run_id: request.ci_run_id.clone(),
            cd_run_id: request.cd_run_id.clone(),
            note: request.note.clone(),
            waiting: staging_wait_text(request.state, &request.revision),
            updated_at: crate::auth::rfc3339(request.updated_at),
        })
    }

    /// `POST /api/sandboxes/{sid}/cd/{env}/staging` (write; SPEC-650 PST-001): the person's
    /// request for the merge-first environment, answered with where it stands.
    ///
    /// # Errors
    /// `NotFound` without access, every refusal of `App::staging_ask`.
    pub async fn cd_staging_ask(
        self: &Arc<Self>,
        ctx: &crate::auth::AuthContext,
        sandbox_id: &str,
        environment: &str,
    ) -> AppResult<yulius_contracts::api::cd::CdStagingRequest> {
        let sid =
            yulius_domain::ids::SandboxId::parse(sandbox_id).map_err(|_| AppError::NotFound)?;
        let (sandbox, _) = self.require_for(ctx, &sid, Action::Work).await?;
        let env = yulius_domain::cd::env_name(environment)
            .ok_or_else(|| AppError::Invalid(super::cd::UNKNOWN_ENV.into()))?;
        let request = self.staging_ask(&sandbox, &env, &ctx.user).await?;
        self.staging_request_view(&request).await
    }

    /// The queue of the merge-first environment `env` for the CD page and `cd_status`: the
    /// verified revision and the newest requests with their run ids (SPEC-575).
    pub(crate) async fn cd_staging_view(
        &self,
        sandbox: &SandboxRecord,
        target: &CdTarget,
    ) -> AppResult<yulius_contracts::api::cd::CdStaging> {
        let store = &self.deps.store;
        let recent = store
            .recent_staging_requests(&sandbox.id, &target.name, 20)
            .await?;
        let mut requests = Vec::with_capacity(recent.len());
        for request in &recent {
            requests.push(self.staging_request_view(request).await?);
        }
        // What staging serves (a rollback moves it back), when a verified request put it there.
        let verified = self.staging_verified_revision(sandbox, target).await?;
        let waiting = recent
            .iter()
            .zip(&requests)
            .find(|(r, _)| is_asked(r) && r.state != StagingState::Superseded)
            .and_then(|(r, shown)| match r.state {
                // A failed deploy waits for a person's press, not for tests (review round 2).
                StagingState::DeployFailed => Some(format!(
                    "The deploy of {} failed: Deploy asks for it again",
                    r.revision.get(..7).unwrap_or(&r.revision)
                )),
                _ => shown.waiting.clone(),
            });
        Ok(yulius_contracts::api::cd::CdStaging {
            verified,
            requests,
            waiting,
            github_wait: self.github_wait(),
        })
    }
}

/// What a pending request of no task waits for, in plain words (SPEC-650 PST-005).
pub(crate) fn staging_wait_text(state: StagingState, revision: &str) -> Option<String> {
    let sha = revision.get(..7).unwrap_or(revision);
    Some(match state {
        StagingState::Testing => format!("Waiting for green tests on {sha}"),
        StagingState::Inherited => {
            format!("Waiting for green tests: {sha} failed; the next merged revision is tested")
        }
        StagingState::Ready => format!("Tests passed on {sha}: it deploys next"),
        StagingState::Deploying => format!("Deploying {sha}"),
        _ => return None,
    })
}

/// `cd_status`'s lines for a merge-first environment's queue.
pub(crate) fn staging_text(
    text: &mut String,
    env: &str,
    staging: &yulius_contracts::api::cd::CdStaging,
) {
    text.push_str(&format!(
        "  staging queue of [cd.{env}] (merge-first: a task's cd_deploy asks for its merged revision, a person's Deploy on the CD page or their Superagent's cd_deploy for the integration head; the full gate runs on exactly it first): verified revision {}\n",
        staging.verified.as_deref().map_or("none yet", short)
    ));
    if let Some(waiting) = &staging.waiting {
        text.push_str(&format!("  {waiting}\n"));
    }
    if let Some(wait) = &staging.github_wait {
        text.push_str(wait);
        text.push('\n');
    }
    for request in staging.requests.iter().take(8) {
        text.push_str(&format!(
            "  - request {} {} revision {}: {}{}{}\n",
            request.id,
            match (&request.asked_by, request.task_number) {
                (Some(who), _) => format!("asked by {who},"),
                (None, None) if request.scheduled => "the schedule's,".to_owned(),
                (None, Some(n)) => format!("task #{n}"),
                (None, None) => "a deleted task's".to_owned(),
            },
            short(&request.revision),
            request.state,
            run_ids_of(request.ci_run_id.as_deref(), request.cd_run_id.as_deref()),
            request
                .note
                .as_deref()
                .map_or_else(String::new, |n| format!(" ({n})")),
        ));
    }
}

/// ", CI run …, CD run …" of a request, for its lines.
fn run_ids(request: &StagingRequestRecord) -> String {
    run_ids_of(request.ci_run_id.as_deref(), request.cd_run_id.as_deref())
}

/// ", CI run …, CD run …" for the ids a request has.
fn run_ids_of(ci: Option<&str>, cd: Option<&str>) -> String {
    let run =
        |label: &str, id: Option<&str>| id.map_or_else(String::new, |id| format!(", {label} {id}"));
    format!("{}{}", run("CI run", ci), run("CD run", cd))
}

/// What a task reads about its staging request's outcome, with the next step.
pub(super) fn staging_outcome_text(request: &StagingRequestRecord) -> String {
    let what = format!(
        "Yulius: staging request {} for revision {} of [cd.{}] is {}",
        request.id,
        short(&request.revision),
        request.environment,
        request.state.as_str()
    );
    // A successful request records what its run verified, even after a later deploy or rollback.
    let detail = if request.state.is_success() {
        format!(
            "{} {}",
            run_ids(request),
            request.note.as_deref().unwrap_or("")
        )
    } else {
        request
            .note
            .as_ref()
            .map_or_else(String::new, |note| format!(": {note}"))
    };
    format!("{what}{detail}. {}", staging_next(request))
}

/// The next step a request's state leaves its task (empty while it is on its way).
fn staging_next(request: &StagingRequestRecord) -> String {
    match request.state {
        StagingState::Inherited => "Staging will serve this request when it verifies a revision containing the fix.".to_owned(),
        StagingState::Verified => {
            "Staging deployed and verified your revision, so this task's staging deployment is done."
                .to_owned()
        }
        StagingState::Included => {
            "The verified staging deployment included your revision. This request is complete.".to_owned()
        }
        StagingState::GateFailed => match &request.ci_run_id {
            Some(run) => format!(
                "Staging did not change. Read the run with ci_status {{\"run_id\": \"{run}\"}}, then follow the working task assigned the repair."
            ),
            None => "Staging did not change. There was no CI run for this revision. The project needs a working task to restore the gate.".to_owned(),
        },
        StagingState::DeployFailed => format!(
            "Read the run with cd_status {{\"run_id\": \"{}\"}}; fix it forward and merge, or cd_deploy again to retry the same revision.",
            request.cd_run_id.as_deref().unwrap_or("?")
        ),
        StagingState::Superseded => {
            "It will not deploy; cd_deploy again for the task's current merged revision if it still needs staging.".to_owned()
        }
        _ => String::new(),
    }
}
