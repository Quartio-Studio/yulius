//! Rollback in CD (SPEC-415, ADR-0263): a person's or an agent's rollback of an environment's
//! part to a kept release, the releases deploys record, the drill after every passed dev-stage
//! deploy, and the gate that keeps production deploys waiting for a proven rollback.
//!
//! A rollback is a CD run like a deploy ([`super::cd`]): the same verified sources, rules of who
//! may run it, containment, secrets, audit and one-run-per-environment. It differs in what it
//! runs: the environment's `rollback` command, once per step with the step's release in
//! `YULIUS_CD_RELEASE`, each followed by its `verify`, from the scripts of the branch's verified
//! head (never promoting). A drill is a rollback Yulius starts itself on dev: back to the previous
//! release and forward to the new one, and it never runs on a production-stage environment.

use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;

use yulius_contracts::api::cd::{CdPart, CdRelease, CdRollback, CdRollbackRequest, CdRun};
use yulius_domain::cd::{self as dom, CdRefusal, CdStage, CdTarget, Requester, env_label};
use yulius_domain::ci::RunState;
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::rollback::{
    CdRunKind, DrillSeen, History, ReleaseKind, ReleaseRecord, RollbackStep, drill_steps,
    production_gate, shipped, short_release,
};

use super::cd::{
    CdActor, CdRequest, CdRunEnd, CdRunRecord, Prepared, mirror_lock, no_promotion, refusal,
    running_refusal, short_sha, shown_requester,
};
use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::coordination::AgentContext;
use crate::delivery::delivery_profile;
use crate::delivery::runner::OutputRing;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;

/// The fixed script of a rollback or a drill: output to the log, stdin closed, then for each
/// step (`part release sha` after the commands) the rollback command with the step in
/// `YULIUS_CD_PART`, `YULIUS_CD_RELEASE` and `YULIUS_CD_RELEASE_SHA`, and the verify command when
/// it succeeded. Each command is its own `/bin/sh -c` argument and each value a positional one:
/// nothing configured or recorded is spliced into this script.
pub(super) const ROLLBACK_WRAPPER: &str = r#"exec >>"$1" 2>&1 </dev/null
rollback=$2
verify=$3
shift 3
while [ "$#" -ge 3 ]; do
  YULIUS_CD_PART=$1 YULIUS_CD_RELEASE=$2 YULIUS_CD_RELEASE_SHA=$3
  export YULIUS_CD_PART YULIUS_CD_RELEASE YULIUS_CD_RELEASE_SHA
  shift 3
  printf '[yulius] rollback: %s%s\n' "${YULIUS_CD_PART:+$YULIUS_CD_PART }" "$YULIUS_CD_RELEASE"
  /bin/sh -c "$rollback" || { s=$?; printf '[yulius] rollback failed with status %s; verify skipped\n' "$s"; exit "$s"; }
  if [ -n "$verify" ]; then
    printf '[yulius] verify: %s\n' "$verify"
    /bin/sh -c "$verify" || { s=$?; printf '[yulius] verify failed with status %s\n' "$s"; exit "$s"; }
  fi
done
printf '[yulius] done\n'"#;

/// Longest marker line read from a log; longer lines are skipped.
const MAX_MARKER_LINE: usize = 4096;
/// Most marker lines read from one log.
const MAX_MARKERS: usize = 256;

/// The steps as people read them: `engine 1.4.1, engine 1.4.2` or `v1`.
pub(super) fn steps_text(steps: &[RollbackStep]) -> String {
    steps
        .iter()
        .map(|step| {
            let release = short_release(&step.release);
            if step.part.is_empty() {
                release.to_owned()
            } else {
                format!("{} {release}", step.part)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The environment and part as people read them: `dev` or `dev engine`.
fn part_label(env: &str, part: &str) -> String {
    if part.is_empty() {
        env_label(env).to_owned()
    } else {
        format!("{} {part}", env_label(env))
    }
}

/// The `::yulius-release` and `::yulius-rollback-barrier` lines of a deploy's whole log, read
/// off the async runtime and bounded (line length and count); none when it cannot be read.
pub(super) async fn release_markers(log: &Path) -> Vec<String> {
    let log = log.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let Ok(file) = std::fs::File::open(&log) else {
            return Vec::new();
        };
        let mut reader = std::io::BufReader::with_capacity(64 * 1024, file);
        let (mut found, mut line, mut overflow) = (Vec::new(), Vec::new(), false);
        let mut chunk = [0_u8; 64 * 1024];
        loop {
            let read = match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            for &byte in &chunk[..read] {
                if byte != b'\n' {
                    if line.len() < MAX_MARKER_LINE {
                        line.push(byte);
                    } else {
                        overflow = true;
                    }
                    continue;
                }
                keep_marker(&mut found, &line, overflow);
                line.clear();
                overflow = false;
            }
            if found.len() >= MAX_MARKERS {
                break;
            }
        }
        keep_marker(&mut found, &line, overflow);
        found.truncate(MAX_MARKERS);
        found
    })
    .await
    .unwrap_or_default()
}

fn keep_marker(found: &mut Vec<String>, line: &[u8], overflow: bool) {
    let text = String::from_utf8_lossy(line);
    let text = text.trim();
    if !overflow
        && (text.starts_with("::yulius-release") || text.starts_with("::yulius-rollback-barrier"))
    {
        found.push(text.to_owned());
    }
}

impl App {
    /// `POST /api/sandboxes/{sid}/cd/{env}/rollback` (write; production: administrator and the
    /// typed slug, as a deploy).
    ///
    /// # Errors
    /// As [`App::cd_deploy`]; `Conflict` without `rollback`, for a release Yulius did not keep,
    /// the one deployed now, one behind a barrier, or a part the environment does not have.
    pub async fn cd_rollback(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sandbox_id: &str,
        environment: &str,
        req: &CdRollbackRequest,
    ) -> AppResult<CdRun> {
        let sid = SandboxId::parse(sandbox_id).map_err(|_| AppError::NotFound)?;
        let (sandbox, level) = self.require(&ctx.user, &sid, Action::Work).await?;
        let env = dom::env_name(environment).ok_or_else(|| {
            AppError::Invalid("the environment is a [cd.<name>] of yulius.toml".into())
        })?;
        let requester = Requester::Person {
            administrator: level.allows(Action::Administer),
            confirmation: req.confirm.as_deref(),
        };
        let run = self
            .cd_start(
                &sandbox,
                &env,
                requester,
                &CdActor::Person(&ctx.user),
                CdRequest::Rollback {
                    part: req.part.as_deref(),
                    release: req.release.as_deref(),
                    confirm_ms: req.confirm_ms,
                },
            )
            .await?;
        Ok(self.cd_run_view(&run, false).await)
    }

    /// Everything a rollback needs, from verified sources: the environment's stage and table at
    /// its branch's verified head (production: the production head, without promotion), the
    /// part, the kept release it goes to and the secrets (RBK-020..023).
    pub(super) async fn cd_prepare_rollback(
        &self,
        sandbox: &SandboxRecord,
        env: &str,
        (requester, actor): (Requester<'_>, &CdActor<'_>),
        (part, release): (Option<&str>, Option<&str>),
        resolved: &mut Option<CdStage>,
    ) -> AppResult<Prepared> {
        let mirror_lock = mirror_lock(&sandbox.id);
        let _mirror = mirror_lock.lock().await;
        let profile = delivery_profile(sandbox);
        self.dlv_refresh_clone(sandbox).await?;
        let token = self.dlv_installation_token(sandbox).await?;
        let integration = self
            .cd_verified_head(sandbox, &profile.integration_branch, &token)
            .await?;
        let integration_config = self.cd_verified_config(sandbox, &integration).await;
        let mut stage = dom::resolve_stage(env, integration_config.as_ref().ok(), None);
        let mut production = None;
        if stage.is_none() {
            let head = self
                .cd_verified_head(sandbox, &profile.production_branch, &token)
                .await?;
            let config = self.cd_verified_config(sandbox, &head).await.ok();
            stage = dom::resolve_stage(env, None, config.as_ref());
            production = Some(head);
        }
        let Some(stage) = stage else {
            return Err(refusal(&CdRefusal::not_defined(env)));
        };
        *resolved = Some(stage);
        dom::may_deploy(stage, requester, &sandbox.slug).map_err(|r| refusal(&r))?;
        let (branch, head, config) = match stage {
            CdStage::Dev => (
                profile.integration_branch.clone(),
                integration,
                integration_config?,
            ),
            CdStage::Prod => {
                let head = match production {
                    Some(head) => head,
                    None => {
                        self.cd_verified_head(sandbox, &profile.production_branch, &token)
                            .await?
                    }
                };
                // A rollback never promotes: production's own table at its head.
                let config = no_promotion(self.cd_verified_config(sandbox, &head).await?);
                (profile.production_branch.clone(), head, config)
            }
        };
        let Some(target) = config.target_of(env, stage).cloned() else {
            return Err(refusal(&CdRefusal::not_defined(env)));
        };
        if target.rollback.is_none() {
            return Err(AppError::Conflict(format!(
                "rollback is not configured for {}: add `rollback` and `migrations` to [cd.{env}] \
                 of yulius.toml on {branch}",
                env_label(env)
            )));
        }
        let part = match part.map(str::trim).filter(|p| !p.is_empty()) {
            Some(named) if target.parts.iter().any(|p| p == named) => named.to_owned(),
            Some(named) => {
                return Err(AppError::Conflict(format!(
                    "{} has no part '{named}'{}",
                    env_label(env),
                    if target.parts.is_empty() {
                        String::new()
                    } else {
                        format!(": its parts are {}", target.parts.join(", "))
                    }
                )));
            }
            None if target.parts.len() > 1 => {
                return Err(AppError::Conflict(format!(
                    "name the part of {} to roll back: {}",
                    env_label(env),
                    target.parts.join(", ")
                )));
            }
            None => target.parts.first().cloned().unwrap_or_default(),
        };
        let records: Vec<ReleaseRecord> = self
            .deps
            .store
            .cd_releases(&sandbox.id, env, stage)
            .await?
            .into_iter()
            .filter(|record| record.part == part)
            .collect();
        let history = History(&records);
        let what = part_label(env, &part);
        let to = match release.map(str::trim).filter(|r| !r.is_empty()) {
            Some(id) => history.find(id).ok_or_else(|| {
                AppError::Conflict(format!(
                    "{id} is not one of the releases Yulius keeps for {what}: pick one the CD page \
                     or cd_status lists"
                ))
            })?,
            None => history.previous().ok_or_else(|| {
                AppError::Conflict(format!("{what} has no earlier release to roll back to"))
            })?,
        };
        let current = history.current();
        if current.is_some_and(|now| now.release == to.release) {
            return Err(AppError::Conflict(format!(
                "{} is already deployed to {what}",
                short_release(&to.release)
            )));
        }
        if let Some(why) = history.blocked(&to.release) {
            return Err(AppError::Conflict(format!("nothing ran: {why}")));
        }
        if self
            .deps
            .store
            .cd_runs(&sandbox.id, super::cd::CD_RUN_LIST_LIMIT)
            .await?
            .iter()
            .any(|run| run.environment == env && run.state == RunState::Running)
        {
            return Err(running_refusal(env));
        }
        self.cd_names_present(sandbox, &target).await?;
        let asker = match actor {
            CdActor::Agent(ctx) => Some(*ctx),
            CdActor::Person(..) => None,
        };
        let why = format!("CD {} rollback", env_label(env));
        let secrets = self
            .cd_secret_values(sandbox, env, stage, &target.secrets, asker, &why)
            .await?;
        let note = format!(
            "rolling {what} from {} to {} (deployed {} by {})",
            current.map_or("nothing", |now| short_release(&now.release)),
            short_release(&to.release),
            rfc3339(to.at),
            shown_requester(&to.requested_by)
        );
        Ok(Prepared {
            stage,
            branch,
            sha: to.sha.clone(),
            target,
            secrets,
            note: Some(note),
            promoted: None,
            kind: CdRunKind::Rollback,
            source: Some(head),
            steps: vec![RollbackStep::to(to)],
        })
    }

    /// Why a production deploy of `sha` is refused for its rollback, if it is (RBK-031): read
    /// against `repo` (the verified mirror for a deploy; the clone for the page).
    pub(super) async fn cd_rollback_gate(
        &self,
        sandbox: &SandboxRecord,
        target: &CdTarget,
        sha: &str,
        repo: &Path,
    ) -> AppResult<Option<String>> {
        if target.stage != CdStage::Prod || target.deploy.is_none() || target.rollback.is_none() {
            return Ok(production_gate(target, None));
        }
        let drill_env = target.drill_env();
        let seen = match self
            .deps
            .store
            .last_cd_run_of_kind(&sandbox.id, drill_env, CdStage::Dev, CdRunKind::Drill)
            .await?
        {
            None => None,
            Some(run) if run.state == RunState::Running => {
                return Ok(Some(format!(
                    "the rollback drill of {drill_env} is running: deploy production once it passed"
                )));
            }
            Some(run) => {
                let git = &self.deps.git;
                let drilled = git.tree_of(repo, &run.sha).await.ok().flatten();
                let deploying = git.tree_of(repo, sha).await.ok().flatten();
                Some(DrillSeen {
                    passed: run.state == RunState::Passed,
                    same_tree: drilled.is_some() && drilled == deploying,
                    sha: run.sha,
                })
            }
        };
        Ok(production_gate(target, seen.as_ref()))
    }

    /// Records what a passed run deployed (RBK-010..012): a deploy's shipped releases (its markers;
    /// the ignored ones said in its output), a rollback's release. A drill ends where it started.
    pub(super) async fn cd_record_releases(
        &self,
        sandbox: &SandboxRecord,
        run: &CdRunRecord,
        target: &CdTarget,
        markers: &[String],
        ring: &mut OutputRing,
    ) {
        let at = self.now();
        let record = |part: String, release: String, sha: String, kind, barrier| ReleaseRecord {
            part,
            release,
            sha,
            kind,
            requested_by: run.requested_by.clone(),
            at,
            barrier,
            run_id: run.id.clone(),
        };
        let records: Vec<ReleaseRecord> = match run.kind {
            CdRunKind::Drill => return,
            CdRunKind::Deploy => {
                let (shipped, ignored) =
                    shipped(markers.iter().map(String::as_str), &target.parts, &run.sha);
                for note in ignored {
                    ring.push_str(&format!("[yulius] ignored {note}\n"));
                }
                shipped
                    .into_iter()
                    .map(|s| {
                        record(
                            s.part,
                            s.release,
                            run.sha.clone(),
                            ReleaseKind::Deploy,
                            s.barrier,
                        )
                    })
                    .collect()
            }
            CdRunKind::Rollback => run
                .steps
                .iter()
                .map(|s| {
                    record(
                        s.part.clone(),
                        s.release.clone(),
                        s.sha.clone(),
                        ReleaseKind::Rollback,
                        None,
                    )
                })
                .collect(),
        };
        for r in &records {
            ring.push_str(&format!(
                "[yulius] {} is now {}{}\n",
                part_label(&run.environment, &r.part),
                r.release,
                r.barrier
                    .as_deref()
                    .map(|why| format!(" (no rollback past it: {why})"))
                    .unwrap_or_default()
            ));
        }
        if let Err(err) = self
            .deps
            .store
            .insert_cd_releases(&sandbox.id, &run.environment, run.stage, &records)
            .await
        {
            tracing::error!(error = %err, run = %run.id, "recording a CD run's releases failed");
            ring.push_str("[yulius] its releases could not be recorded\n");
        }
    }

    /// Starts the drill of a passed dev-stage deploy whose table has `rollback` (RBK-030): each
    /// part it shipped goes back to its previous release and forward again, each step verified.
    /// Never for a production-stage environment, a rollback or a drill. A drill that cannot start
    /// is recorded as an error with why, so the page and the gate say so.
    pub(super) async fn cd_start_drill(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        deployed: &CdRunRecord,
        target: &CdTarget,
    ) {
        if deployed.stage != CdStage::Dev
            || deployed.kind != CdRunKind::Deploy
            || target.stage != CdStage::Dev
            || target.rollback.is_none()
        {
            return;
        }
        let records = match self
            .deps
            .store
            .cd_releases(&sandbox.id, &deployed.environment, CdStage::Dev)
            .await
        {
            Ok(records) => records,
            Err(err) => {
                tracing::warn!(error = %err, "reading releases for a drill failed");
                return;
            }
        };
        let mut steps = Vec::new();
        for part in target.part_names() {
            let mine: Vec<ReleaseRecord> =
                records.iter().filter(|r| r.part == part).cloned().collect();
            if mine.first().is_some_and(|r| r.run_id == deployed.id) {
                steps.extend(drill_steps(History(&mine)));
            }
        }
        if steps.is_empty() {
            return;
        }
        let run = CdRunRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: sandbox.id.clone(),
            environment: deployed.environment.clone(),
            stage: CdStage::Dev,
            branch: deployed.branch.clone(),
            sha: deployed.sha.clone(),
            state: RunState::Running,
            reason: None,
            requested_by: "yulius".to_owned(),
            task_id: None,
            deploy: target.rollback.clone(),
            verify: target.verify.clone(),
            note: Some(format!(
                "rollback drill after deploy {} by {}",
                deployed.id,
                shown_requester(&deployed.requested_by)
            )),
            output: String::new(),
            exit_code: None,
            started_at: self.now(),
            ended_at: None,
            kind: CdRunKind::Drill,
            source: None,
            steps,
        };
        let target_name = format!("{}/{}", sandbox.slug, run.environment);
        let started = self.now();
        let permit = match self.deploy_permit() {
            Ok(permit) => permit,
            Err(err) => {
                return self
                    .cd_drill_not_started(sandbox, run, &err.public_message())
                    .await;
            }
        };
        let why = format!("CD {} rollback drill", env_label(&run.environment));
        let secrets = match self.cd_names_present(sandbox, target).await {
            Ok(()) => {
                self.cd_secret_values(
                    sandbox,
                    &run.environment,
                    CdStage::Dev,
                    &target.secrets,
                    None,
                    &why,
                )
                .await
            }
            Err(err) => Err(err),
        };
        let secrets = match secrets {
            Ok(secrets) => secrets,
            Err(err) => {
                return self
                    .cd_drill_not_started(sandbox, run, &err.public_message())
                    .await;
            }
        };
        let prepared = Prepared {
            stage: CdStage::Dev,
            branch: run.branch.clone(),
            sha: run.sha.clone(),
            target: target.clone(),
            secrets,
            note: run.note.clone(),
            promoted: None,
            kind: CdRunKind::Drill,
            source: None,
            steps: run.steps.clone(),
        };
        let recorded = self
            .cd_record_and_execute(
                sandbox,
                run,
                prepared,
                ("yulius".to_owned(), target_name),
                (started, None),
            )
            .await;
        drop(permit);
        if let Err(err) = recorded {
            tracing::warn!(error = %err, "the rollback drill did not start");
        }
    }

    /// Records a drill that could not start as an error saying why (its audit entry too).
    async fn cd_drill_not_started(&self, sandbox: &SandboxRecord, run: CdRunRecord, why: &str) {
        let target = format!("{}/{}", sandbox.slug, run.environment);
        let reason = format!("the rollback drill could not start: {why}");
        if self.deps.store.insert_cd_run(&run).await.is_ok() {
            let _ = self
                .deps
                .store
                .finish_cd_run(
                    &run.id,
                    &CdRunEnd {
                        state: RunState::Error,
                        reason: Some(reason.clone()),
                        exit_code: None,
                        output: format!("[yulius] {reason}\n"),
                        ended_at: self.now(),
                        usage: None,
                    },
                )
                .await;
        }
        self.dlv_audit("yulius", "cd.drill", &target, &reason).await;
    }

    /// An environment's rollback for the page (RBK-040): its command and migrations line, each
    /// part's kept releases, its drill and, for production, why a deploy of `deploying` (as the
    /// clone shows it) would be refused for its rollback.
    pub(super) async fn cd_rollback_view(
        &self,
        sandbox: &SandboxRecord,
        target: &CdTarget,
        deploying: Option<&str>,
    ) -> AppResult<CdRollback> {
        let (name, stage) = (target.name.as_str(), target.stage);
        let records = self
            .deps
            .store
            .cd_releases(&sandbox.id, name, stage)
            .await?;
        let mut parts = Vec::new();
        for part in target.part_names() {
            let mine: Vec<ReleaseRecord> =
                records.iter().filter(|r| r.part == part).cloned().collect();
            let history = History(&mine);
            let current = history.current().map(|r| r.release.clone());
            parts.push(CdPart {
                previous: history.previous().map(|r| r.release.clone()),
                releases: history
                    .releases()
                    .into_iter()
                    .map(|r| CdRelease {
                        release: r.release.clone(),
                        commit: r.sha.clone(),
                        deployed_at: rfc3339(r.at),
                        requested_by: shown_requester(&r.requested_by),
                        current: current.as_ref() == Some(&r.release),
                        blocked: history.blocked(&r.release),
                        barrier: r.barrier.clone(),
                    })
                    .collect(),
                current,
                name: part,
            });
        }
        let drill_environment = match stage {
            CdStage::Dev => name.to_owned(),
            CdStage::Prod => target.drill_env().to_owned(),
        };
        let drill = match self
            .deps
            .store
            .last_cd_run_of_kind(
                &sandbox.id,
                &drill_environment,
                CdStage::Dev,
                CdRunKind::Drill,
            )
            .await?
        {
            Some(run) => Some(self.cd_run_view(&run, false).await),
            None => None,
        };
        let blocked = match (stage, deploying) {
            (CdStage::Prod, Some(sha)) => {
                self.cd_rollback_gate(sandbox, target, sha, &self.sandbox_repo(&sandbox.id))
                    .await?
            }
            (CdStage::Prod, None) => production_gate(target, None),
            (CdStage::Dev, _) => None,
        };
        Ok(CdRollback {
            command: target.rollback.clone(),
            migrations: target.migrations.clone(),
            parts,
            drill_environment,
            drill,
            blocked,
        })
    }

    /// `cd_rollback`: a dev-stage rollback for a task's agent under its owner's write access.
    ///
    /// # Errors
    /// `Invalid` arguments; production-stage environments are `Forbidden`; every refusal of
    /// [`App::cd_rollback`].
    pub(crate) async fn tool_cd_rollback(
        self: &Arc<Self>,
        ctx: &AgentContext,
        args: &serde_json::Value,
    ) -> AppResult<serde_json::Value> {
        let text = |key: &str| args.get(key).and_then(serde_json::Value::as_str);
        let name = text("environment")
            .ok_or_else(|| AppError::Invalid("environment is required (dev)".into()))?;
        let env = dom::env_name(name).ok_or_else(|| {
            AppError::Invalid("the environment is a [cd.<name>] of yulius.toml".into())
        })?;
        let owner = self
            .deps
            .store
            .user(&ctx.task.owner)
            .await?
            .filter(|u| u.disabled_at.is_none())
            .ok_or_else(|| {
                AppError::Forbidden(
                    "the person who owns this task is no longer active; nobody can roll back for it"
                        .into(),
                )
            })?;
        let (sandbox, _) = self
            .require(&owner, &ctx.task.sandbox_id, Action::Work)
            .await?;
        let run = self
            .cd_start(
                &sandbox,
                &env,
                Requester::Agent,
                &CdActor::Agent(ctx),
                CdRequest::Rollback {
                    part: text("part"),
                    release: text("release"),
                    confirm_ms: None,
                },
            )
            .await?;
        let view = self.cd_run_view(&run, false).await;
        let text = format!(
            "Rolling {} back: run {} to {} (the scripts of {} at {}). Start `yulius wait` in the \
             background: it returns when the run ends; then read it with cd_status {{run_id}}.",
            env_label(&run.environment),
            run.id,
            steps_text(&run.steps),
            run.branch,
            short_sha(run.source.as_deref().unwrap_or(&run.sha))
        );
        Ok(serde_json::json!({"text": text, "run": view}))
    }
}

/// An environment's rollback as `cd_status` prints it.
pub(super) fn rollback_text(out: &mut String, rollback: &CdRollback, stage: &str) {
    let Some(command) = &rollback.command else {
        out.push_str(
            "  rollback: not configured (add `rollback` and `migrations` to its table; production \
             deploys need a proven rollback)\n",
        );
        if let Some(blocked) = &rollback.blocked {
            out.push_str(&format!("  production deploys refused: {blocked}\n"));
        }
        return;
    };
    out.push_str(&format!("  rollback: {command}\n"));
    if let Some(migrations) = &rollback.migrations {
        out.push_str(&format!("  migrations: {migrations}\n"));
    }
    for part in &rollback.parts {
        let releases: Vec<String> = part
            .releases
            .iter()
            .map(|r| {
                let mut shown = short_release(&r.release).to_owned();
                if r.current {
                    shown.push_str(" (current)");
                } else if r.blocked.is_some() {
                    shown.push_str(" (blocked)");
                }
                shown
            })
            .collect();
        let name = if part.name.is_empty() {
            String::new()
        } else {
            format!(" of {}", part.name)
        };
        out.push_str(&format!(
            "  releases{name}: {}\n",
            if releases.is_empty() {
                "none yet".to_owned()
            } else {
                releases.join(", ")
            }
        ));
    }
    match &rollback.drill {
        Some(drill) => out.push_str(&format!(
            "  drill: {} at {} ({}){}\n",
            drill.state,
            short_sha(&drill.commit),
            rollback.drill_environment,
            drill
                .reason
                .as_deref()
                .map(|r| format!(": {r}"))
                .unwrap_or_default()
        )),
        None if stage == "prod" || rollback.parts.iter().any(|p| !p.releases.is_empty()) => {
            out.push_str(&format!(
                "  drill: none yet on {} (it follows a deploy with an earlier release)\n",
                rollback.drill_environment
            ));
        }
        None => {}
    }
    if stage == "prod" {
        match &rollback.blocked {
            Some(blocked) => out.push_str(&format!("  production deploys refused: {blocked}\n")),
            None => out.push_str("  rollback proven: production may deploy\n"),
        }
    }
}
