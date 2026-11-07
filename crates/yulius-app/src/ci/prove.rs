//! A production deploy that proves its rollback on dev first (SPEC-438, ADR-0282).
//!
//! A person's production deploy refused only because the newest drill of its drill environment
//! ran at another tree (or none ran) deploys that environment at the commit instead, as the same
//! person. The production deploy waits here, in memory, for that deploy and the drill it starts:
//! once the drill passed it runs as the person with the commit they confirmed; otherwise an ended
//! production run in state error says why it did not deploy. A restart forgets the wait.

use std::sync::Arc;

use yulius_domain::cd::{CdConfig, CdStage, CdTarget, Requester, env_label};
use yulius_domain::ci::RunState;
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::Action;
use yulius_domain::rollback::CdRunKind;

use super::cd::{CdActor, CdRequest, CdRunRecord, short_sha};
use crate::context::App;
use crate::delivery::delivery_profile;
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
#[derive(Debug, Clone)]
pub(crate) struct ProductionWait {
    sandbox_id: SandboxId,
    /// The production-stage environment.
    environment: String,
    drill_env: String,
    /// The commit the drill environment deploys.
    dev_sha: String,
    /// The commit production deploys (the person's `expect`).
    expect: String,
    user_id: UserId,
    login: String,
    /// The drill of the deploy, once it started.
    drill_run: Option<String>,
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

    /// Waits for a deploy of `env` at `sha` (or its drill) in `sandbox`.
    fn proved_by(&self, sandbox: &SandboxId, env: &str, sha: &str) -> bool {
        self.sandbox_id == *sandbox && self.drill_env == env && self.dev_sha == sha
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
            .find(|w| w.sandbox_id == *sandbox && w.environment == env)
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
        if let Some(waiting) = self.cd_production_waiting(&sandbox.id, env) {
            return Err(AppError::Conflict(format!("nothing ran: {waiting}")));
        }
        let actor = CdActor::Person(user);
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
        let wait = ProductionWait {
            sandbox_id: sandbox.id.clone(),
            environment: env.to_owned(),
            drill_env: proof.drill_env.clone(),
            dev_sha: proof.dev_sha.clone(),
            expect: proof.expect.clone(),
            user_id: user.id.clone(),
            login: App::dlv_actor_login(user),
            drill_run: None,
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
            if let Some(other) = waits
                .iter()
                .find(|w| w.sandbox_id == sandbox.id && w.environment == env)
            {
                return Err(AppError::Conflict(format!(
                    "nothing ran: {}",
                    other.sentence()
                )));
            }
            waits.push(wait);
        }
        let dev = match requester {
            Requester::Person { administrator, .. } => Requester::Person {
                administrator,
                confirmation: None,
            },
            other => other,
        };
        let started = self
            .cd_begin(
                sandbox,
                &proof.drill_env,
                dev,
                &actor,
                CdRequest::Deploy {
                    expected: Some(&proof.dev_sha),
                    confirm_ms: None,
                    note: Some(&note),
                },
                None,
            )
            .await;
        match started {
            Ok(run) => Ok(run),
            Err(err) => {
                self.cd_take_wait(|w| w.sandbox_id == sandbox.id && w.environment == env);
                Err(AppError::Conflict(format!(
                    "{}; {} could not be deployed to prove it: {}",
                    refused.public_message(),
                    proof.drill_env,
                    err.public_message()
                )))
            }
        }
    }

    /// How `target`'s refused production deploy of `sha` can be proved on its drill environment:
    /// a dev-stage table at the integration head with `deploy` and `rollback`, whose head has the
    /// tree of `sha` (SPEC-438 PDF-001).
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

    /// The deploy `deployed` starts drill `drill`: a production deploy waiting for the deploy
    /// waits for the drill.
    pub(super) fn cd_proof_drilling(
        &self,
        sandbox: &SandboxId,
        deployed: &CdRunRecord,
        drill: &str,
    ) {
        if let Ok(mut waits) = self.ci.production_waits.lock() {
            for wait in waits.iter_mut().filter(|w| {
                w.drill_run.is_none() && w.proved_by(sandbox, &deployed.environment, &deployed.sha)
            }) {
                wait.drill_run = Some(drill.to_owned());
            }
        }
    }

    /// The proof of a production deploy waiting for `env` at `sha` ended without a drill that
    /// ran: production does not deploy, and says `why`.
    pub(super) async fn cd_proof_ended(
        &self,
        sandbox: &SandboxRecord,
        env: &str,
        sha: &str,
        why: &str,
    ) {
        for wait in self.cd_take_wait(|w| w.proved_by(&sandbox.id, env, sha)) {
            self.cd_production_not_deployed(sandbox, &wait, why, true)
                .await;
        }
    }

    /// After a run ended (SPEC-438 PDF-002/003): a dev deploy that did not pass, or passed without
    /// a drill following, ends the waits for it; a drill ends the wait for it, deploying
    /// production once it passed.
    pub(super) async fn cd_proof_after(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        run: &CdRunRecord,
        state: RunState,
    ) {
        let (env, sha) = (run.environment.as_str(), run.sha.as_str());
        match (run.kind, run.stage) {
            (CdRunKind::Deploy, CdStage::Dev) => {
                // A drill that started took the wait over; one left here had none follow.
                let waiting = |w: &ProductionWait| {
                    w.drill_run.is_none() && w.proved_by(&sandbox.id, env, sha)
                };
                let why = if state == RunState::Passed {
                    format!(
                        "no rollback drill of {env} followed its deploy at {}: {env} needs an \
                         earlier release to go back to",
                        short_sha(sha)
                    )
                } else {
                    format!("the deploy of {env} at {} {}", short_sha(sha), ended(state))
                };
                for wait in self.cd_take_wait(waiting) {
                    self.cd_production_not_deployed(sandbox, &wait, &why, true)
                        .await;
                }
            }
            (CdRunKind::Drill, _) => {
                let drilled = |w: &ProductionWait| w.drill_run.as_deref() == Some(run.id.as_str());
                for wait in self.cd_take_wait(drilled) {
                    if state == RunState::Passed {
                        self.cd_production_proved(sandbox, &wait).await;
                    } else {
                        let why = format!(
                            "the rollback drill of {env} at {} {}",
                            short_sha(sha),
                            ended(state)
                        );
                        self.cd_production_not_deployed(sandbox, &wait, &why, true)
                            .await;
                    }
                }
            }
            _ => {}
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
    /// read again and the commit they confirmed (SPEC-438 PDF-002).
    async fn cd_production_proved(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        wait: &ProductionWait,
    ) {
        let user = match self.deps.store.user(&wait.user_id).await {
            Ok(Some(user)) if user.disabled_at.is_none() => user,
            _ => {
                let why = format!("{} can no longer deploy it", wait.login);
                return self
                    .cd_production_not_deployed(sandbox, wait, &why, true)
                    .await;
            }
        };
        let level = match self.require(&user, &sandbox.id, Action::Work).await {
            Ok((_, level)) => level,
            Err(err) => {
                return self
                    .cd_production_not_deployed(sandbox, wait, &err.public_message(), true)
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
                    note: None,
                },
            )
            .await;
        if let Err(err) = started {
            // The refusal of a production deploy is audited already.
            let why = err.public_message();
            let why = why.strip_prefix("nothing ran: ").unwrap_or(&why).to_owned();
            self.cd_production_not_deployed(sandbox, wait, &why, false)
                .await;
        }
    }

    /// Records that the waiting production deploy did not deploy: an ended run in state error
    /// saying why, audited unless its refusal was (SPEC-438 PDF-003).
    async fn cd_production_not_deployed(
        &self,
        sandbox: &SandboxRecord,
        wait: &ProductionWait,
        why: &str,
        audit: bool,
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
        if audit {
            let target = format!("{}/{}", sandbox.slug, wait.environment);
            self.dlv_audit(&wait.login, "cd.refused", &target, &reason)
                .await;
        }
    }
}
