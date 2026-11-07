//! A production deploy that proves its rollback on dev first (SPEC-438, ADR-0282).
//!
//! A person's production deploy refused only because the newest drill of its drill environment
//! ran at another tree (or none ran) deploys that environment at the commit instead, as the same
//! person. The production deploy waits here, in memory, bound to that dev run and then to the
//! drill it starts: once the drill passed it runs as the person with the commit they confirmed;
//! otherwise an ended production run in state error says why it did not deploy. The wait holds a
//! deploy permit, so no update restarts the server under it; a restart for another reason forgets
//! it.

use std::sync::Arc;

use yulius_domain::cd::{self as dom, CdConfig, CdStage, CdTarget, Requester, env_label};
use yulius_domain::ci::RunState;
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::Action;
use yulius_domain::rollback::{CdRunKind, ReleaseKind};

use super::cd::{CdActor, CdRequest, CdRunRecord, Proving, refusal, short_sha};
use crate::context::App;
use crate::delivery::delivery_profile;
use crate::deploy_gate::DeployPermit;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, UserRecord};

/// How a refused production deploy can be proved on dev: deploy `drill_env` at `dev_sha`, whose
/// tree is the tree of `expect`, the commit production deploys.
#[derive(Debug, Clone)]
pub(crate) struct ProveOnDev {
    drill_env: String,
    dev_sha: String,
    expect: String,
}

/// A production deploy waiting for its proof.
pub(crate) struct ProductionWait {
    sandbox_id: SandboxId,
    /// The production-stage environment.
    environment: String,
    drill_env: String,
    /// The commit production deploys (the person's `expect`).
    expect: String,
    user_id: UserId,
    login: String,
    /// The dev deploy it waits for.
    dev_run: String,
    /// The drill of that deploy, once it was prepared.
    drill_run: Option<String>,
    /// Held until production started or did not: an update waits for it.
    _permit: DeployPermit,
}

impl ProductionWait {
    fn sentence(&self) -> String {
        let label = env_label(&self.environment);
        let mut chars = label.chars();
        let label: String = chars
            .next()
            .map(|c| c.to_uppercase().chain(chars).collect())
            .unwrap_or_default();
        format!(
            "{label} deploys at {} once the rollback drill of {} passes (asked by {})",
            short_sha(&self.expect),
            self.drill_env,
            self.login
        )
    }

    fn of(&self, sandbox: &SandboxId, env: &str) -> bool {
        self.sandbox_id == *sandbox && self.environment == env
    }
}

/// How a run that did not pass ended, after "the deploy of dev at abc1234".
fn ended(state: RunState) -> &'static str {
    match state {
        RunState::Failed => "failed",
        RunState::Cancelled => "was cancelled",
        RunState::Error => "ended with an error",
        _ => "did not pass",
    }
}

impl App {
    /// The production deploy of `env` waiting in `sandbox`, as the page says it.
    pub(super) fn cd_production_waiting(&self, sandbox: &SandboxId, env: &str) -> Option<String> {
        let waits = self.ci.production_waits.lock().ok()?;
        waits
            .iter()
            .find(|w| w.of(sandbox, env))
            .map(ProductionWait::sentence)
    }

    /// A person's deploy (`cd_deploy`): a production deploy refused only for a rollback a dev
    /// deploy can prove deploys the drill environment instead and waits for its drill (SPEC-438
    /// PDF-001); the answer is that run.
    pub(super) async fn cd_start_or_prove(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        env: &str,
        requester: Requester<'_>,
        user: &UserRecord,
        request: CdRequest<'_>,
    ) -> AppResult<CdRunRecord> {
        let actor = CdActor::Person(user);
        if let Some(waiting) = self.cd_production_waiting(&sandbox.id, env) {
            // Who first, as any production refusal; audited as one.
            let target = format!("{}/{env}", sandbox.slug);
            let refused = match dom::fixed_stage(env)
                .map(|stage| dom::may_deploy(stage, requester, &sandbox.slug))
            {
                Some(Err(refused)) => refusal(&refused),
                _ => AppError::Conflict(format!("nothing ran: {waiting}")),
            };
            self.dlv_audit(
                &actor.label(),
                "cd.refused",
                &target,
                &refused.public_message(),
            )
            .await;
            return Err(refused);
        }
        let mut proof = None;
        let refused = match self
            .cd_begin(sandbox, env, requester, &actor, request, Some(&mut proof))
            .await
        {
            Ok(run) => return Ok(run),
            Err(err) => err,
        };
        let Some(proof) = proof else {
            return Err(refused);
        };
        let administrator = match requester {
            Requester::Person { administrator, .. } => administrator,
            Requester::Agent => false,
        };
        // Detached from the request (ADR-0233): a client that goes away while dev is prepared
        // never leaves a wait without its run.
        let (app, sandbox, user, env) = (
            Arc::clone(self),
            sandbox.clone(),
            user.clone(),
            env.to_owned(),
        );
        let refused = refused.public_message();
        Self::run_detached(async move {
            app.cd_prove(&sandbox, &env, (&user, administrator), proof, &refused)
                .await
        })
        .await
    }

    /// Registers the wait and starts the dev deploy it is bound to; a dev deploy that does not
    /// start removes the wait and says why with production's refusal.
    async fn cd_prove(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        env: &str,
        (user, administrator): (&UserRecord, bool),
        proof: ProveOnDev,
        refused: &str,
    ) -> AppResult<CdRunRecord> {
        let permit = self.deploy_permit()?;
        let dev_run = self.deps.crypto.new_id();
        let wait = ProductionWait {
            sandbox_id: sandbox.id.clone(),
            environment: env.to_owned(),
            drill_env: proof.drill_env.clone(),
            expect: proof.expect.clone(),
            user_id: user.id.clone(),
            login: App::dlv_actor_login(user),
            dev_run: dev_run.clone(),
            drill_run: None,
            _permit: permit,
        };
        let note = format!(
            "deployed to prove the rollback of {label}: {label} deploys at {} once this deploy's \
             rollback drill passes (asked by {})",
            short_sha(&wait.expect),
            wait.login,
            label = env_label(env),
        );
        // Registered before the deploy starts, so its end always finds it.
        {
            let mut waits = self
                .ci
                .production_waits
                .lock()
                .map_err(|_| AppError::Internal("the production waits are poisoned".into()))?;
            if let Some(other) = waits.iter().find(|w| w.of(&sandbox.id, env)) {
                return Err(AppError::Conflict(format!(
                    "nothing ran: {}",
                    other.sentence()
                )));
            }
            waits.push(wait);
        }
        let started = self
            .cd_begin(
                sandbox,
                &proof.drill_env,
                Requester::Person {
                    administrator,
                    confirmation: None,
                },
                &CdActor::Person(user),
                CdRequest::Deploy {
                    expected: Some(&proof.dev_sha),
                    confirm_ms: None,
                    proving: Some(Proving {
                        run_id: &dev_run,
                        note: &note,
                    }),
                },
                None,
            )
            .await;
        match started {
            Ok(run) => Ok(run),
            Err(err) => {
                // Nothing of it runs: no end will come for this wait.
                self.cd_take_wait(|w| w.dev_run == dev_run);
                Err(AppError::Conflict(format!(
                    "{refused}; {} could not be deployed to prove it: {}",
                    proof.drill_env,
                    err.public_message()
                )))
            }
        }
    }

    /// How `target`'s refused production deploy of `sha` can be proved on its drill environment
    /// (SPEC-438 PDF-001): a dev-stage table at the integration head with `deploy` and `rollback`,
    /// whose head has the tree of `sha`, and an earlier release from another commit for its
    /// drill to go back to.
    pub(super) async fn cd_dev_proof(
        &self,
        sandbox: &SandboxRecord,
        target: &CdTarget,
        (integration, tables): (&str, &CdConfig),
        sha: &str,
    ) -> Option<ProveOnDev> {
        let drill_env = target.drill_env();
        let dev = tables.target_of(drill_env, CdStage::Dev)?;
        if dev.deploy.is_none() || dev.rollback.is_none() {
            return None;
        }
        // Nothing deployed yet, or deployed at this commit already: no drill would follow.
        let records = self
            .deps
            .store
            .cd_releases(&sandbox.id, drill_env, CdStage::Dev)
            .await
            .ok()?;
        let newest = records.iter().find(|r| r.kind == ReleaseKind::Deploy)?;
        if newest.sha == integration {
            return None;
        }
        if integration != sha {
            let (git, mirror) = (&self.deps.git, self.cd_mirror(&sandbox.id));
            let dev_tree = git.tree_of(&mirror, integration).await.ok().flatten()?;
            let tree = git.tree_of(&mirror, sha).await.ok().flatten()?;
            if dev_tree != tree {
                return None;
            }
        }
        Some(ProveOnDev {
            drill_env: drill_env.to_owned(),
            dev_sha: integration.to_owned(),
            expect: sha.to_owned(),
        })
    }

    /// The deploy `deployed` prepares drill `drill`: a production deploy waiting for the deploy
    /// waits for the drill.
    pub(super) fn cd_proof_drilling(&self, deployed: &CdRunRecord, drill: &str) {
        if let Ok(mut waits) = self.ci.production_waits.lock() {
            for wait in waits.iter_mut().filter(|w| w.dev_run == deployed.id) {
                wait.drill_run = Some(drill.to_owned());
            }
        }
    }

    /// Drill `drill` does not run: the production deploy waiting for it does not deploy, and
    /// says `why`.
    pub(super) async fn cd_proof_ended(&self, sandbox: &SandboxRecord, drill: &str, why: &str) {
        for wait in self.cd_take_wait(|w| w.drill_run.as_deref() == Some(drill)) {
            self.cd_production_not_deployed(sandbox, &wait, why).await;
        }
    }

    /// After a run ended (SPEC-438 PDF-002/003): a dev deploy that did not pass, or passed without
    /// a drill prepared after it, ends the wait bound to it; a drill ends the wait for it,
    /// deploying production once it passed.
    pub(super) async fn cd_proof_after(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        run: &CdRunRecord,
        state: RunState,
    ) {
        let (env, sha) = (run.environment.as_str(), short_sha(&run.sha));
        match run.kind {
            CdRunKind::Deploy => {
                // A drill that was prepared took the wait over.
                let why = if state == RunState::Passed {
                    format!("no rollback drill of {env} followed its deploy at {sha}")
                } else {
                    format!("the deploy of {env} at {sha} {}", ended(state))
                };
                for wait in self.cd_take_wait(|w| w.dev_run == run.id && w.drill_run.is_none()) {
                    self.cd_production_not_deployed(sandbox, &wait, &why).await;
                }
            }
            CdRunKind::Drill => {
                for wait in self.cd_take_wait(|w| w.drill_run.as_deref() == Some(run.id.as_str())) {
                    if state == RunState::Passed {
                        self.cd_production_proved(sandbox, &wait).await;
                    } else {
                        let why = format!("the rollback drill of {env} at {sha} {}", ended(state));
                        self.cd_production_not_deployed(sandbox, &wait, &why).await;
                    }
                }
            }
            CdRunKind::Rollback => {}
        }
    }

    /// Removes and returns the waits `which` matches.
    fn cd_take_wait(&self, which: impl Fn(&ProductionWait) -> bool) -> Vec<ProductionWait> {
        let Ok(mut waits) = self.ci.production_waits.lock() else {
            return Vec::new();
        };
        let (taken, kept) = std::mem::take(&mut *waits)
            .into_iter()
            .partition(|w| which(w));
        *waits = kept;
        taken
    }

    /// The proof passed: the production deploy runs as the person who asked, with their access
    /// read again and the commit they confirmed (SPEC-438 PDF-002). The wait's permit is held
    /// until it started or did not.
    async fn cd_production_proved(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        wait: &ProductionWait,
    ) {
        let user = match self.deps.store.user(&wait.user_id).await {
            Ok(Some(user)) if user.disabled_at.is_none() => user,
            _ => {
                let why = format!("{} can no longer deploy it", wait.login);
                return self.cd_production_not_deployed(sandbox, wait, &why).await;
            }
        };
        let level = match self.require(&user, &sandbox.id, Action::Work).await {
            Ok((_, level)) => level,
            Err(err) => {
                return self
                    .cd_production_not_deployed(sandbox, wait, &err.public_message())
                    .await;
            }
        };
        let started = self
            .cd_start(
                sandbox,
                &wait.environment,
                Requester::Person {
                    administrator: level.allows(Action::Administer),
                    // Typed when the person asked for this deploy.
                    confirmation: Some(&sandbox.slug),
                },
                &CdActor::Person(&user),
                CdRequest::Deploy {
                    expected: Some(&wait.expect),
                    confirm_ms: None,
                    proving: None,
                },
            )
            .await;
        if let Err(err) = started {
            let why = err.public_message();
            let why = why.strip_prefix("nothing ran: ").unwrap_or(&why).to_owned();
            self.cd_production_not_deployed(sandbox, wait, &why).await;
        }
    }

    /// Records that the waiting production deploy did not deploy: an ended run in state error
    /// saying why, audited `cd.refused` (SPEC-438 PDF-003).
    async fn cd_production_not_deployed(
        &self,
        sandbox: &SandboxRecord,
        wait: &ProductionWait,
        why: &str,
    ) {
        let now = self.now();
        let reason = format!("{} did not deploy: {why}", env_label(&wait.environment));
        let run = CdRunRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: sandbox.id.clone(),
            environment: wait.environment.clone(),
            stage: CdStage::Prod,
            branch: delivery_profile(sandbox).production_branch,
            sha: wait.expect.clone(),
            state: RunState::Error,
            reason: Some(reason.clone()),
            requested_by: wait.login.clone(),
            task_id: None,
            deploy: None,
            verify: None,
            note: None,
            output: format!("[yulius] {reason}\n"),
            exit_code: None,
            started_at: now,
            ended_at: Some(now),
            kind: CdRunKind::Deploy,
            source: None,
            steps: Vec::new(),
        };
        if let Err(err) = self.deps.store.insert_cd_run(&run).await {
            tracing::warn!(error = %err, "recording a production deploy that did not run failed");
        }
        let target = format!("{}/{}", sandbox.slug, wait.environment);
        self.dlv_audit(&wait.login, "cd.refused", &target, &reason)
            .await;
    }
}
