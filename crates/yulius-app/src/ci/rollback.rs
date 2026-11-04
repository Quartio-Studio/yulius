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

use yulius_contracts::api::cd::{
    CdEnvironment, CdPart, CdRelease, CdRollback, CdRollbackRequest, CdRun,
};
use yulius_domain::cd::{self as dom, CdRefusal, CdStage, CdTarget, Requester, env_label};
use yulius_domain::ci::RunState;
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::rollback::{
    CdRunKind, DrillSeen, History, ReleaseKind, ReleaseRecord, RollbackStep, drill_steps,
    earlier_barrier, production_gate, shipped, short_release,
};

use super::cd::{
    CdActor, CdRequest, CdRunRecord, Prepared, mirror_lock, no_promotion, refusal, running_refusal,
    short_sha, shown_requester,
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
/// it succeeded. After each rollback command that succeeded it prints `::yulius-rolled-back <n>`
/// (the step's index), so a run that fails later still says what the environment runs (review
/// M1). Each command is its own `/bin/sh -c` argument and each value a positional one: nothing
/// configured or recorded is spliced into this script.
pub(super) const ROLLBACK_WRAPPER: &str = r#"exec >>"$1" 2>&1 </dev/null
rollback=$2
verify=$3
step=0
shift 3
while [ "$#" -ge 3 ]; do
  YULIUS_CD_PART=$1 YULIUS_CD_RELEASE=$2 YULIUS_CD_RELEASE_SHA=$3
  export YULIUS_CD_PART YULIUS_CD_RELEASE YULIUS_CD_RELEASE_SHA
  shift 3
  printf '[yulius] rollback: %s%s\n' "${YULIUS_CD_PART:+$YULIUS_CD_PART }" "$YULIUS_CD_RELEASE"
  /bin/sh -c "$rollback" || { s=$?; printf '[yulius] rollback failed with status %s; verify skipped\n' "$s"; exit "$s"; }
  printf '::yulius-rolled-back %s\n' "$step"
  step=$((step + 1))
  if [ -n "$verify" ]; then
    printf '[yulius] verify: %s\n' "$verify"
    /bin/sh -c "$verify" || { s=$?; printf '[yulius] verify failed with status %s\n' "$s"; exit "$s"; }
  fi
done
printf '[yulius] done\n'"#;

/// Longest marker line read from a log; longer lines are skipped.
const MAX_MARKER_LINE: usize = 4096;
/// Most marker lines kept from one log: the last ones.
const MAX_MARKERS: usize = 256;
/// Most bytes read from one log: a run's output bound.
const MAX_LOG_BYTES: u64 = 256 << 20;

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

/// The `::yulius-release`, `::yulius-rollback-barrier` and `::yulius-rolled-back` lines of a run's
/// whole log, the last [`MAX_MARKERS`], read off the async runtime. Opened as `LogTail::open`
/// does (review H1): a link is never followed and only a regular file is read, so a FIFO or a
/// device put in the log's place is never waited on; at most [`MAX_LOG_BYTES`] are read. None
/// when it cannot be read.
pub(super) async fn release_markers(log: &Path) -> Vec<String> {
    let log = log.to_path_buf();
    tokio::task::spawn_blocking(move || {
        use std::os::unix::fs::OpenOptionsExt as _;
        let flags = nix::fcntl::OFlag::O_NOFOLLOW | nix::fcntl::OFlag::O_NONBLOCK;
        let Ok(file) = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(flags.bits())
            .open(&log)
        else {
            return Vec::new();
        };
        if !file.metadata().is_ok_and(|meta| meta.is_file()) {
            return Vec::new();
        }
        let mut reader = file.take(MAX_LOG_BYTES);
        let mut found = std::collections::VecDeque::new();
        let (mut line, mut overflow) = (Vec::new(), false);
        let mut chunk = vec![0_u8; 64 * 1024];
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
        }
        keep_marker(&mut found, &line, overflow);
        Vec::from(found)
    })
    .await
    .unwrap_or_default()
}

fn keep_marker(found: &mut std::collections::VecDeque<String>, line: &[u8], overflow: bool) {
    let text = String::from_utf8_lossy(line);
    let text = text.trim();
    if !overflow
        && [
            "::yulius-release",
            "::yulius-rollback-barrier",
            "::yulius-rolled-back",
        ]
        .iter()
        .any(|marker| text.starts_with(marker))
    {
        if found.len() == MAX_MARKERS {
            found.pop_front();
        }
        found.push_back(text.to_owned());
    }
}

/// The step each part of a rollback or a drill reached: of the steps the wrapper said it rolled
/// to (`::yulius-rolled-back <n>`, in order), the last of each part (review M1).
fn reached_steps(steps: &[RollbackStep], markers: &[String]) -> Vec<RollbackStep> {
    let done = markers
        .iter()
        .filter_map(|m| m.strip_prefix("::yulius-rolled-back "))
        .filter_map(|n| n.trim().parse::<usize>().ok())
        .filter(|n| *n < steps.len())
        .max();
    let Some(done) = done else {
        return Vec::new();
    };
    let mut reached: Vec<RollbackStep> = Vec::new();
    for step in &steps[..=done] {
        reached.retain(|r| r.part != step.part);
        reached.push(step.clone());
    }
    reached
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

    /// Records what a run left deployed (RBK-010..012): a passed deploy's shipped releases (its
    /// markers; the ignored ones said in its output), and the release each part of a rollback, or
    /// of a drill that did not pass, last rolled to, verified or not (review M1). A passed drill
    /// ends where it started. Release ids and barrier reasons are redacted of the sandbox's agent
    /// secrets before they are kept (review M4): an id that held one is ignored.
    pub(super) async fn cd_record_releases(
        &self,
        sandbox: &SandboxRecord,
        (run, state): (&CdRunRecord, RunState),
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
        let mut records = Vec::new();
        // What is recorded so far: earlier barriers, and where a part stands (review round 3: a
        // failed read is said, never silently taken as no history).
        let known = match self
            .deps
            .store
            .cd_releases(&sandbox.id, &run.environment, run.stage)
            .await
        {
            Ok(known) => known,
            Err(err) => {
                tracing::warn!(error = %err, run = %run.id, "reading a CD run's releases failed");
                ring.push_str("[yulius] the releases recorded so far could not be read\n");
                Vec::new()
            }
        };
        match run.kind {
            CdRunKind::Deploy if state == RunState::Passed => {
                let earlier = &known;
                let (shipped, ignored) =
                    shipped(markers.iter().map(String::as_str), &target.parts, &run.sha);
                for note in ignored {
                    ring.push_str(&format!("[yulius] ignored {note}\n"));
                }
                for s in shipped {
                    let release = self.redact_for_sandbox(&sandbox.id, &s.release).await;
                    if release != s.release {
                        ring.push_str("[yulius] ignored a release id that holds a secret\n");
                        continue;
                    }
                    // A release deployed again keeps its first deploy's barrier (review round 2).
                    let barrier = match &s.barrier {
                        Some(why) => Some(self.redact_for_sandbox(&sandbox.id, why).await),
                        None => earlier_barrier(earlier, &s.part, &release),
                    };
                    records.push(record(
                        s.part,
                        release,
                        run.sha.clone(),
                        ReleaseKind::Deploy,
                        barrier,
                    ));
                }
            }
            CdRunKind::Deploy => {}
            CdRunKind::Drill if state == RunState::Passed => {}
            CdRunKind::Rollback | CdRunKind::Drill => {
                let current = &known;
                let mut reached = reached_steps(&run.steps, markers);
                // A passed rollback whose markers could not be read went to every step.
                if reached.is_empty()
                    && run.kind == CdRunKind::Rollback
                    && state == RunState::Passed
                {
                    let all: Vec<String> = (0..run.steps.len())
                        .map(|n| format!("::yulius-rolled-back {n}"))
                        .collect();
                    reached = reached_steps(&run.steps, &all);
                }
                for step in reached {
                    let now = current.iter().find(|r| r.part == step.part);
                    // A drill that failed after going back records where it left the part.
                    if run.kind == CdRunKind::Drill
                        && now.is_some_and(|r| r.release == step.release)
                    {
                        continue;
                    }
                    records.push(record(
                        step.part,
                        step.release,
                        step.sha,
                        ReleaseKind::Rollback,
                        None,
                    ));
                }
            }
        }
        if records.is_empty() {
            return;
        }
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
    /// part with an earlier release, shipped by this deploy or not (review M2), goes back to it and
    /// forward again, each step verified. Never for a production-stage environment, a rollback or
    /// a drill. A drill that cannot start (its releases unreadable, a secret, an update, another
    /// run of the environment) is recorded as an error saying why, with its audit entry, so the
    /// page and the gate say so (review, low).
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
        let mut run = CdRunRecord {
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
            steps: Vec::new(),
        };
        let records = match self
            .deps
            .store
            .cd_releases(&sandbox.id, &deployed.environment, CdStage::Dev)
            .await
        {
            Ok(records) => records,
            Err(err) => {
                return self
                    .cd_drill_not_started(sandbox, run, &err.public_message())
                    .await;
            }
        };
        for part in target.part_names() {
            let mine: Vec<ReleaseRecord> =
                records.iter().filter(|r| r.part == part).cloned().collect();
            run.steps.extend(drill_steps(History(&mine)));
        }
        if run.steps.is_empty() {
            return;
        }
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
                run.clone(),
                prepared,
                ("yulius".to_owned(), target_name),
                (started, None),
            )
            .await;
        drop(permit);
        // Another run of the environment started first: nothing of the drill was recorded.
        if let Err(AppError::Conflict(_)) = recorded {
            self.cd_drill_not_started(
                sandbox,
                run,
                "another run of the environment was running; deploy it again for a drill",
            )
            .await;
        } else if let Err(err) = recorded {
            tracing::warn!(error = %err, "the rollback drill did not start");
        }
    }

    /// Records a drill that could not start as an ended error saying why (never running, so a
    /// running run of the environment does not stand in its way), with its audit entry.
    async fn cd_drill_not_started(&self, sandbox: &SandboxRecord, run: CdRunRecord, why: &str) {
        let target = format!("{}/{}", sandbox.slug, run.environment);
        let reason = format!("the rollback drill could not start: {why}");
        let ended = CdRunRecord {
            state: RunState::Error,
            reason: Some(reason.clone()),
            output: format!("[yulius] {reason}\n"),
            ended_at: Some(self.now()),
            ..run
        };
        if let Err(err) = self.deps.store.insert_cd_run(&ended).await {
            tracing::warn!(error = %err, "recording a drill that could not start failed");
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

/// Why an environment that runs no deploy command needs no rollback (SPEC-418 RCF-001).
fn no_rollback_needed(env: &CdEnvironment) -> &'static str {
    if env.stage == "prod" && env.promote {
        "promoting is the release: nothing deployed here can be redeployed; undo a release by \
         reverting it and promoting again"
    } else {
        "it runs no deploy command; add `rollback` and `migrations` with one"
    }
}

/// An environment's rollback as `cd_status` prints it.
pub(super) fn rollback_text(out: &mut String, env: &CdEnvironment) {
    let (rollback, stage) = (&env.rollback, env.stage.as_str());
    if rollback.command.is_none() && env.deploy.is_none() {
        // Nothing Yulius deploys here could be redeployed (SPEC-418, ADR-0264).
        out.push_str(&format!(
            "  rollback: not needed ({})\n",
            no_rollback_needed(env)
        ));
        return;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// Review H1 of SPEC-415: the markers come from the regular file the run wrote, never through
    /// a link or from a FIFO or a device the deploy put in its place, and the reading ends.
    fn env(stage: &str, deploy: Option<&str>, promote: bool) -> CdEnvironment {
        CdEnvironment {
            name: stage.into(),
            label: stage.into(),
            stage: stage.into(),
            state: if deploy.is_some() || promote {
                "ready"
            } else {
                "empty"
            }
            .into(),
            branch: if stage == "prod" { "main" } else { "dev" }.into(),
            head: None,
            problem: None,
            about: None,
            deploy: deploy.map(Into::into),
            verify: None,
            promote,
            promotes_from: None,
            secrets: Vec::new(),
            states: Vec::new(),
            deployed: None,
            last_run: None,
            rollback: CdRollback {
                drill_environment: "dev".into(),
                ..CdRollback::default()
            },
        }
    }

    /// SPEC-418 RCF-001/003: an environment that runs no deploy command needs no rollback, and
    /// says why; one that deploys without `rollback` is still "not configured".
    #[test]
    fn environments_that_deploy_nothing_need_no_rollback() {
        let said = |env: &CdEnvironment| {
            let mut out = String::new();
            rollback_text(&mut out, env);
            out
        };
        let promoting = said(&env("prod", None, true));
        assert!(
            promoting.contains("rollback: not needed (promoting is the release")
                && promoting.contains("reverting it and promoting again"),
            "{promoting}"
        );
        let empty = said(&env("dev", None, false));
        assert!(
            empty.contains("rollback: not needed (it runs no deploy command"),
            "{empty}"
        );
        for out in [&promoting, &empty] {
            assert!(!out.contains("not configured"), "{out}");
            assert!(!out.contains("need a proven rollback"), "{out}");
        }
        let deploying = said(&env("prod", Some("scripts/cd/deploy.sh prod"), false));
        assert!(
            deploying.contains("rollback: not configured"),
            "{deploying}"
        );
    }

    #[tokio::test]
    async fn markers_are_read_only_from_the_regular_log() -> TestResult {
        let dir = tempfile::tempdir()?;
        let log = dir.path().join("log");
        std::fs::write(&log, "x\n::yulius-release v1\n  ::yulius-rolled-back 0  \n")?;
        assert_eq!(
            release_markers(&log).await,
            ["::yulius-release v1", "::yulius-rolled-back 0"]
        );
        let elsewhere = dir.path().join("elsewhere");
        std::fs::write(&elsewhere, "::yulius-release secret\n")?;
        let linked = dir.path().join("linked");
        std::os::unix::fs::symlink(&elsewhere, &linked)?;
        assert!(
            release_markers(&linked).await.is_empty(),
            "a link is not followed"
        );
        let zero = dir.path().join("zero");
        std::os::unix::fs::symlink("/dev/zero", &zero)?;
        assert!(release_markers(&zero).await.is_empty());
        let fifo = dir.path().join("fifo");
        nix::unistd::mkfifo(&fifo, nix::sys::stat::Mode::S_IRWXU)?;
        let read = tokio::time::timeout(std::time::Duration::from_secs(20), release_markers(&fifo))
            .await?;
        assert!(read.is_empty(), "a FIFO is never waited on");
        assert!(
            release_markers(&dir.path().join("missing"))
                .await
                .is_empty()
        );
        Ok(())
    }

    /// The last markers win when there are more than are kept; overlong lines are skipped.
    #[tokio::test]
    async fn the_last_markers_are_kept() -> TestResult {
        let dir = tempfile::tempdir()?;
        let log = dir.path().join("log");
        let mut text = format!("::yulius-release {}\n", "a".repeat(MAX_MARKER_LINE));
        for n in 0..300 {
            text.push_str(&format!("::yulius-release v{n}\n"));
        }
        text.push_str("::yulius-release last");
        std::fs::write(&log, text)?;
        let found = release_markers(&log).await;
        assert_eq!(found.len(), MAX_MARKERS);
        assert_eq!(
            found.last().map(String::as_str),
            Some("::yulius-release last")
        );
        assert_eq!(
            found.first().map(String::as_str),
            Some("::yulius-release v45")
        );
        Ok(())
    }

    /// Review M1: the steps a rollback or a drill completed, from the wrapper's markers.
    #[test]
    fn completed_steps_come_from_the_wrapper_markers() {
        let step = |part: &str, release: &str| RollbackStep {
            part: part.into(),
            release: release.into(),
            sha: format!("sha-{release}"),
        };
        let steps = [
            step("engine", "e1"),
            step("engine", "e2"),
            step("infra", "i1"),
        ];
        let markers = |lines: &[&str]| lines.iter().map(|l| (*l).to_owned()).collect::<Vec<_>>();
        let reached = |lines: &[&str]| -> Vec<String> {
            reached_steps(&steps, &markers(lines))
                .into_iter()
                .map(|s| s.release)
                .collect()
        };
        assert!(reached(&[]).is_empty());
        assert_eq!(reached(&["::yulius-rolled-back 0"]), ["e1"]);
        assert_eq!(
            reached(&[
                "::yulius-rolled-back 0",
                "::yulius-rolled-back 1",
                "::yulius-rolled-back 2"
            ]),
            ["e2", "i1"],
            "the last step of each part"
        );
        assert_eq!(
            reached(&["::yulius-rolled-back 7", "::yulius-rolled-back x"]),
            Vec::<String>::new()
        );
    }
}
