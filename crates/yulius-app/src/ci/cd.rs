//! CD (deployment): runs of a project's environments (SPEC-134, SPEC-257, ADR-0109, ADR-0110,
//! ADR-0195).
//!
//! A CD run deploys one environment from its protected branch: Yulius fetches, asks GitHub for
//! the branch head, takes exactly that commit from the sandbox clone into a mirror only Yulius
//! writes (every object checked, replacement refs ignored), reads `[cd.<env>]` from `yulius.toml`
//! there, writes the commit's files into the run's private directory and runs the `deploy`
//! command and then the `verify` command through the process host. An environment's stage
//! (`dev`, `prod`) carries its rules: a dev-stage one reads the integration branch; a
//! production-stage one reads the commit it deploys (the integration head decides whether it
//! promotes first; after a promotion the new production head is read again). "Dev" and
//! "production" below mean the stages.
//!
//! Trust contract (review C1, H1, H2 of SPEC-134):
//! - who may deploy is [`yulius_domain::cd::authorize`]: dev is a person's (write access) or an
//!   agent's; production only an administrator's who typed the project's slug, never an agent's;
//! - nothing agents can write changes what a run executes: the sandbox clone, the CI checkouts
//!   and the sandbox directory are not read for a deploy. A run's files (the commit's tree, its
//!   `HOME`, its log) live in its own run directory and its cache in `<data>/cd/<sandbox>/`, bound
//!   only into that run's mount namespace;
//! - dev runs inside the sandbox's containment (agents build and use dev); production runs
//!   outside it, on the host's read-only system with its own root, so nothing an agent installed
//!   in the sandbox runs with production's secrets;
//! - a run's environment has no task token, provider credential or GitHub token; it has the
//!   real values of the secrets its table names that a person allowed for that environment (dev:
//!   outside the proxy, ADR-0105; production: for production deploys, an allowance of its own);
//! - stored output is redacted of every agent secret of the sandbox;
//! - one running run per environment; a run left running by a restart has its process host
//!   terminated and is recorded as `error`, never run again by itself;
//! - every run is audited before it starts (no audit entry, no run), and its end; refused
//!   production deploys are audited too.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::watch;
use yulius_contracts::api::cd::{
    CdAgentState, CdDeploy, CdEnvironment, CdOverview, CdRun, CdSecret,
};
use yulius_domain::cd::{
    self as dom, CdConfig, CdRefusal, CdStage, CdState, CdTarget, Requester, env_label,
};
use yulius_domain::ci::RunState;
use yulius_domain::delivery::DeliveryProfile;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::spec_format::{MAX_CONFIG_BYTES, parse_yulius_toml_cd};
use yulius_domain::time::Timestamp;

use super::OUTPUT_BYTES;
use super::exec::{Ended, Sink, ci_join};
use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::coordination::AgentContext;
use crate::delivery::delivery_profile;
use crate::delivery::runner::OutputRing;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, Secret, UserRecord};
use crate::ports::session::{Containment, LaunchPlan};
use crate::vault::holds::StateHold;
use crate::vault::states::{CdRestored, CdWriter};

/// Runs listed per sandbox.
pub const CD_RUN_LIST_LIMIT: u32 = 20;
/// Runs kept per environment of a sandbox.
pub const CD_KEEP_RUNS: u32 = 50;
/// Runs older than this are deleted (each environment's newest passed run stays).
pub const CD_RETAIN_MS: i64 = 14 * 24 * 60 * 60 * 1000;

/// The fixed script of a run: output to the log, stdin closed, then the deploy command and, when
/// it succeeded, the verify command, each as its own `/bin/sh -c` argument (the configured text
/// is never spliced into this script). An empty command is skipped.
const CD_WRAPPER: &str = r#"exec >>"$1" 2>&1 </dev/null
if [ -n "$2" ]; then
  printf '[yulius] deploy: %s\n' "$2"
  /bin/sh -c "$2" || { s=$?; printf '[yulius] deploy failed with status %s; verify skipped\n' "$s"; exit "$s"; }
fi
if [ -n "$3" ]; then
  printf '[yulius] verify: %s\n' "$3"
  /bin/sh -c "$3" || { s=$?; printf '[yulius] verify failed with status %s\n' "$s"; exit "$s"; }
fi
printf '[yulius] done\n'"#;

/// One CD run (table `cd_runs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdRunRecord {
    /// Id.
    pub id: String,
    /// Sandbox.
    pub sandbox_id: SandboxId,
    /// Environment (`[cd.<name>]`).
    pub environment: String,
    /// Its stage when it ran.
    pub stage: CdStage,
    /// Branch deployed.
    pub branch: String,
    /// Commit deployed.
    pub sha: String,
    /// `running`, `passed`, `failed`, `error` or `cancelled`.
    pub state: RunState,
    /// Why it errored or was cancelled.
    pub reason: Option<String>,
    /// A login, or `task:<number>`.
    pub requested_by: String,
    /// The task whose agent asked.
    pub task_id: Option<TaskId>,
    /// Deploy command run.
    pub deploy: Option<String>,
    /// Verify command run.
    pub verify: Option<String>,
    /// What happened first (the promotion).
    pub note: Option<String>,
    /// Output tail, redacted.
    pub output: String,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Start.
    pub started_at: Timestamp,
    /// End.
    pub ended_at: Option<Timestamp>,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdRunEnd {
    /// `passed`, `failed`, `error` or `cancelled`.
    pub state: RunState,
    /// Why it errored or was cancelled.
    pub reason: Option<String>,
    /// Exit code.
    pub exit_code: Option<i32>,
    /// Final output, redacted.
    pub output: String,
    /// End.
    pub ended_at: Timestamp,
    /// What it used, when its process host measured it (SPEC-154).
    pub usage: Option<yulius_domain::stages::ResourceUse>,
}

/// Who asks for a deploy, as the application sees them.
pub(crate) enum CdActor<'a> {
    /// A signed-in person.
    Person(&'a UserRecord),
    /// A task's agent.
    Agent(&'a AgentContext),
}

impl CdActor<'_> {
    fn label(&self) -> String {
        match self {
            Self::Person(user) => App::dlv_actor_login(user),
            Self::Agent(ctx) => ctx.actor(),
        }
    }
}

/// What a checked deploy runs.
struct Prepared {
    stage: CdStage,
    branch: String,
    sha: String,
    target: CdTarget,
    secrets: Vec<(String, String)>,
    note: Option<String>,
    /// When the promotion it made started and ended (SPEC-137).
    promoted: Option<(Timestamp, Timestamp)>,
}

/// Longest confirmation wait recorded, in milliseconds.
const MAX_CONFIRM_MS: u64 = 60 * 60 * 1000;

/// The protected branches as the clone shows them (the page; never what a deploy runs).
struct Shown {
    integration: Option<String>,
    integration_config: Result<CdConfig, String>,
    production: Option<String>,
    /// Read at the production head, without promotion (only the integration head decides one).
    production_config: Result<CdConfig, String>,
}

/// An environment's configuration as the clone shows it (the page; never what a deploy runs).
struct ShownSource {
    branch: String,
    head: Option<String>,
    config: Result<CdConfig, String>,
    /// Production with `promote`: the integration head it would promote.
    promotes_from: Option<String>,
}

/// Who asked, as shown (`task:12` reads `task #12`).
fn shown_requester(requested_by: &str) -> String {
    match requested_by.strip_prefix("task:") {
        Some(number) => format!("task #{number}"),
        None => requested_by.to_owned(),
    }
}

/// A refusal of the rule as an application error: who may not is `Forbidden`, a wrong
/// confirmation `Invalid`, nothing to run `Conflict`.
fn refusal(refused: &CdRefusal) -> AppError {
    match refused {
        CdRefusal::ProductionIsAPersons | CdRefusal::ProductionNeedsAdministrator => {
            AppError::Forbidden(refused.to_string())
        }
        CdRefusal::ConfirmationMismatch { .. } => AppError::Invalid(refused.to_string()),
        CdRefusal::NotDefined(..) | CdRefusal::NothingToRun(..) => {
            AppError::Conflict(refused.to_string())
        }
    }
}

/// The ring's text for storing: when its start was cut, the cut line goes too, so no secret
/// value split by the cut is left where redaction cannot recognise it (review L5).
fn whole_lines(ring: &OutputRing) -> String {
    let text = ring.text();
    if ring.dropped() == 0 {
        return text;
    }
    match text.split_once('\n') {
        Some((_, rest)) => format!("[yulius] earlier output was cut\n{rest}"),
        None => "[yulius] earlier output was cut\n".to_owned(),
    }
}

impl App {
    /// `<data>/cd/<sandbox>`: Yulius's own CD files, outside every sandbox.
    pub(super) fn cd_private_dir(&self, sandbox: &SandboxId) -> PathBuf {
        self.config.data_dir.join("cd").join(sandbox.as_str())
    }

    fn cd_mirror(&self, sandbox: &SandboxId) -> PathBuf {
        self.cd_private_dir(sandbox).join("source.git")
    }

    fn cd_run_dir(&self, run_id: &str) -> PathBuf {
        self.config
            .data_dir
            .join("runs")
            .join(format!("cd-{run_id}"))
    }

    /// `[cd.dev]` and `[cd.prod]` of `yulius.toml` at `rev`: none without the file, `Err` with
    /// the problem when it is invalid.
    pub(crate) async fn cd_config_at(&self, repo: &Path, rev: &str) -> Result<CdConfig, String> {
        let text = match self.deps.git.show_file(repo, rev, "yulius.toml").await {
            Ok(Some(text)) => text,
            Ok(None) => return Ok(CdConfig::default()),
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
        parse_yulius_toml_cd(&text).map_err(|err| err.to_string())
    }

    /// For the page: the protected branches' heads and tables in the clone as last fetched
    /// (CDC-003). A deploy never uses this: it reads the verified mirror ([`App::cd_prepare`]).
    async fn cd_shown(&self, sandbox: &SandboxRecord, profile: &DeliveryProfile) -> Shown {
        let repo = self.sandbox_repo(&sandbox.id);
        let head = |branch: String| {
            let repo = repo.clone();
            async move {
                self.deps
                    .git
                    .resolve_commit(&repo, &format!("origin/{branch}"))
                    .await
                    .ok()
                    .flatten()
            }
        };
        let integration = head(profile.integration_branch.clone()).await;
        let integration_config = match &integration {
            Some(sha) => self.cd_config_at(&repo, sha).await,
            None => Err(format!(
                "branch '{}' has no head in Yulius's clone yet",
                profile.integration_branch
            )),
        };
        let production = head(profile.production_branch.clone()).await;
        let production_config = match &production {
            Some(sha) => self.cd_config_at(&repo, sha).await.map(no_promotion),
            None => Ok(CdConfig::default()),
        };
        Shown {
            integration,
            integration_config,
            production,
            production_config,
        }
    }

    /// Where the environment `name` of `stage` comes from in what [`App::cd_shown`] read.
    fn cd_shown_source(
        shown: &Shown,
        profile: &DeliveryProfile,
        name: &str,
        stage: CdStage,
    ) -> ShownSource {
        if stage == CdStage::Dev {
            return ShownSource {
                branch: profile.integration_branch.clone(),
                head: shown.integration.clone(),
                config: shown.integration_config.clone(),
                promotes_from: None,
            };
        }
        let promotes = shown.integration_config.as_ref().is_ok_and(|config| {
            config
                .target_of(name, CdStage::Prod)
                .is_some_and(|target| target.promote)
        });
        ShownSource {
            branch: profile.production_branch.clone(),
            head: shown.production.clone(),
            config: if promotes {
                shown.integration_config.clone()
            } else {
                shown.production_config.clone()
            },
            promotes_from: promotes.then(|| shown.integration.clone()).flatten(),
        }
    }

    async fn cd_run_view(&self, run: &CdRunRecord, with_output: bool) -> CdRun {
        CdRun {
            run_id: run.id.clone(),
            environment: run.environment.clone(),
            stage: run.stage.as_str().to_owned(),
            state: run.state.as_str().to_owned(),
            branch: run.branch.clone(),
            commit: run.sha.clone(),
            requested_by: shown_requester(&run.requested_by),
            deploy: run.deploy.clone(),
            verify: run.verify.clone(),
            note: run.note.clone(),
            reason: run.reason.clone(),
            exit_code: run.exit_code,
            started_at: rfc3339(run.started_at),
            ended_at: run.ended_at.map(rfc3339),
            output: with_output.then(|| run.output.clone()),
        }
    }

    /// The CD of `sandbox` as its protected branches define it, with recent runs (SPEC-134
    /// CDT-001, CDT-005). Reads the clone as last fetched (the CI sync fetches it every minute).
    pub(crate) async fn cd_overview_of(
        &self,
        sandbox: &SandboxRecord,
        level: AccessLevel,
    ) -> AppResult<CdOverview> {
        let profile = delivery_profile(sandbox);
        let runs = self
            .deps
            .store
            .cd_runs(&sandbox.id, CD_RUN_LIST_LIMIT)
            .await?;
        let shown = self.cd_shown(sandbox, &profile).await;
        let stored_states = self.deps.store.agent_states(&sandbox.id).await?;
        let listed = dom::environments_of(
            shown.integration_config.as_ref().ok(),
            shown.production_config.as_ref().ok(),
        );
        let mut environments = Vec::with_capacity(listed.len());
        for (name, stage) in listed {
            let source = Self::cd_shown_source(&shown, &profile, &name, stage);
            let (target, problem) = match &source.config {
                Ok(config) => (config.target_of(&name, stage).cloned(), None),
                Err(problem) => (None, Some(problem.clone())),
            };
            let state = match &source.config {
                Ok(config) => config.state(&name, stage),
                Err(_) => CdState::NotDefined,
            };
            let secrets = match &target {
                Some(target) => self
                    .cd_secret_states(&sandbox.id, stage, &target.secrets)
                    .await?
                    .into_iter()
                    .map(|(name, state)| CdSecret {
                        name,
                        state: state.as_str().to_owned(),
                    })
                    .collect(),
                None => Vec::new(),
            };
            let states = target
                .iter()
                .flat_map(|target| target.states.iter())
                .map(|state| CdAgentState {
                    name: state.clone(),
                    version: stored_states
                        .iter()
                        .find(|stored| &stored.name == state)
                        .map(|stored| stored.version),
                    held_by: self.vault.holds.holder(sandbox.id.as_str(), state),
                })
                .collect();
            let deployed = self
                .deps
                .store
                .last_passed_cd_run(&sandbox.id, &name, stage)
                .await?
                .map(|run| run.sha);
            let last_run = match runs
                .iter()
                .find(|run| run.environment == name && run.stage == stage)
            {
                Some(run) => Some(self.cd_run_view(run, false).await),
                None => None,
            };
            let target = target.unwrap_or_else(|| CdTarget::new(&name, stage));
            environments.push(CdEnvironment {
                label: env_label(&name).to_owned(),
                name,
                stage: stage.as_str().to_owned(),
                state: state.as_str().to_owned(),
                branch: source.branch,
                head: source.head,
                problem,
                about: target.about,
                deploy: target.deploy,
                verify: target.verify,
                promote: target.promote,
                promotes_from: source.promotes_from,
                secrets,
                states,
                deployed,
                last_run,
            });
        }
        let mut views = Vec::new();
        for run in &runs {
            views.push(self.cd_run_view(run, false).await);
        }
        Ok(CdOverview {
            slug: sandbox.slug.clone(),
            environments,
            runs: views,
            can_deploy_dev: level.allows(Action::Work),
            can_deploy_prod: level.allows(Action::Administer),
        })
    }

    /// `GET /api/sandboxes/{sid}/cd` (read).
    ///
    /// # Errors
    /// `NotFound` without read access.
    pub async fn cd_overview(&self, ctx: &AuthContext, sandbox_id: &str) -> AppResult<CdOverview> {
        let sid = SandboxId::parse(sandbox_id).map_err(|_| AppError::NotFound)?;
        let (sandbox, level) = self.require(&ctx.user, &sid, Action::View).await?;
        self.cd_overview_of(&sandbox, level).await
    }

    /// `POST /api/sandboxes/{sid}/cd/{env}/deploy` (write; production: administrator and the
    /// typed slug, ADR-0110).
    ///
    /// # Errors
    /// `Invalid` for an unknown environment or a wrong confirmation, `Forbidden` below the
    /// access production needs, `Conflict` for what cannot run now (nothing defined, secrets, a
    /// running run, a promotion that cannot complete).
    pub async fn cd_deploy(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sandbox_id: &str,
        environment: &str,
        req: &CdDeploy,
    ) -> AppResult<CdRun> {
        let sid = SandboxId::parse(sandbox_id).map_err(|_| AppError::NotFound)?;
        let (sandbox, level) = self.require(&ctx.user, &sid, Action::Work).await?;
        let env =
            dom::env_name(environment).ok_or_else(|| AppError::Invalid(UNKNOWN_ENV.into()))?;
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
                (req.expect.as_deref(), req.confirm_ms),
            )
            .await?;
        Ok(self.cd_run_view(&run, false).await)
    }

    /// A deploy of `env` for a task's agent (`cd_deploy`) under its owner's write access.
    ///
    /// # Errors
    /// As [`App::cd_deploy`]; a production-stage environment is always `Forbidden`.
    pub(crate) async fn cd_deploy_for_task(
        self: &Arc<Self>,
        ctx: &AgentContext,
        env: &str,
    ) -> AppResult<CdRunRecord> {
        let store = &self.deps.store;
        let owner = store
            .user(&ctx.task.owner)
            .await?
            .filter(|u| u.disabled_at.is_none())
            .ok_or_else(|| {
                AppError::Forbidden(
                    "the person who owns this task is no longer active; nobody can deploy for it"
                        .into(),
                )
            })?;
        let (sandbox, _) = self
            .require(&owner, &ctx.task.sandbox_id, Action::Work)
            .await?;
        self.cd_start(
            &sandbox,
            env,
            Requester::Agent,
            &CdActor::Agent(ctx),
            (None, None),
        )
        .await
    }

    /// Checks everything, promotes when production says so, records the run and starts it.
    async fn cd_start(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        env: &str,
        requester: Requester<'_>,
        actor: &CdActor<'_>,
        (expected, confirm_ms): (Option<&str>, Option<u64>),
    ) -> AppResult<CdRunRecord> {
        let requested = self.now();
        // A playground has no deploy: its Git stays on the server (SPEC-099).
        crate::playground::refuse_in_playground(sandbox, "A deploy")?;
        let target_name = format!("{}/{env}", sandbox.slug);
        // Who before anything else: an agent never learns more of production than the refusal.
        // A named environment's stage comes from the verified tables: checked again there.
        let fixed = dom::fixed_stage(env);
        if let Some(Err(refused)) =
            fixed.map(|stage| dom::may_deploy(stage, requester, &sandbox.slug))
        {
            self.dlv_audit(
                &actor.label(),
                "cd.refused",
                &target_name,
                &refused.to_string(),
            )
            .await;
            return Err(refusal(&refused));
        }
        // What the checks resolve the stage to: a production-stage refusal is audited, as
        // production's always was; a dev-stage failure is not a refused production deploy.
        let mut resolved = fixed;
        let prepared = match self
            .cd_prepare(sandbox, env, requester, actor, expected, &mut resolved)
            .await
        {
            Ok(prepared) => prepared,
            Err(err) => {
                if resolved == Some(CdStage::Prod) {
                    self.dlv_audit(
                        &actor.label(),
                        "cd.refused",
                        &target_name,
                        &err.public_message(),
                    )
                    .await;
                }
                return Err(err);
            }
        };
        let asker = match actor {
            CdActor::Agent(ctx) => Some(*ctx),
            CdActor::Person(..) => None,
        };
        let run = CdRunRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: sandbox.id.clone(),
            environment: env.to_owned(),
            stage: prepared.stage,
            branch: prepared.branch.clone(),
            sha: prepared.sha.clone(),
            state: RunState::Running,
            reason: None,
            requested_by: actor.label(),
            task_id: asker.map(|ctx| ctx.task.id.clone()),
            deploy: prepared.target.deploy.clone(),
            verify: prepared.target.verify.clone(),
            note: prepared.note.clone(),
            output: String::new(),
            exit_code: None,
            started_at: self.now(),
            ended_at: None,
        };
        self.deps
            .store
            .insert_cd_run(&run)
            .await
            .map_err(|err| match err {
                AppError::Conflict(_) => running_refusal(env),
                other => other,
            })?;
        self.cd_start_spans(&run, requested, prepared.promoted, confirm_ms)
            .await;
        let detail = format!(
            "run {} of {} at {}; deploy: {}; verify: {}; secrets: [{}]; states: [{}]{}",
            run.id,
            run.branch,
            run.sha,
            prepared.target.deploy.as_deref().unwrap_or("-"),
            prepared.target.verify.as_deref().unwrap_or("-"),
            prepared.target.secrets.join(" "),
            prepared.target.states.join(" "),
            run.note
                .as_deref()
                .map(|n| format!("; {n}"))
                .unwrap_or_default(),
        );
        // The audit entry is part of the trust contract: no entry, no run.
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &actor.label(),
                "cd.deploy",
                Some(&target_name),
                Some(&detail),
            )
            .await
        {
            let _ = self
                .deps
                .store
                .finish_cd_run(
                    &run.id,
                    &CdRunEnd {
                        state: RunState::Error,
                        reason: Some("the audit entry could not be written".into()),
                        exit_code: None,
                        output: "[yulius] not started: the audit entry could not be written\n"
                            .into(),
                        ended_at: self.now(),
                        usage: None,
                    },
                )
                .await;
            return Err(err);
        }
        let (tx, rx) = watch::channel(None);
        if let Ok(mut executing) = self.ci.executing.lock() {
            executing.insert(run.id.clone(), tx);
        }
        let app = Arc::clone(self);
        let (sandbox, started) = (sandbox.clone(), run.clone());
        tokio::spawn(async move {
            app.cd_execute(
                sandbox,
                started,
                prepared.secrets,
                (prepared.target.states, prepared.target.variables),
                rx,
            )
            .await;
        });
        Ok(run)
    }

    /// What came before a run's deploy (SPEC-137 STG-301/302): the person's confirmation of a
    /// production deploy as the page measured it, the checks from the request to the run (the
    /// verified commit, its table, the secrets) and the promotion they made.
    async fn cd_start_spans(
        &self,
        run: &CdRunRecord,
        requested: Timestamp,
        promoted: Option<(Timestamp, Timestamp)>,
        confirm_ms: Option<u64>,
    ) {
        let span = |key: &str, kind: &str, name: &str, (from, to): (Timestamp, Timestamp)| {
            crate::stages::NewSpan {
                sandbox_id: run.sandbox_id.clone(),
                subject_kind: yulius_domain::stages::SubjectKind::CdRun,
                subject_id: run.id.clone(),
                key: key.to_owned(),
                parent_key: None,
                kind: kind.to_owned(),
                name: name.to_owned(),
                environment: Some(run.environment.clone()),
                job: None,
                started_at: from,
                ended_at: Some(to),
                outcome: Some("passed".to_owned()),
                detail: None,
            }
        };
        if run.stage == CdStage::Prod
            && let Some(ms) = confirm_ms.filter(|ms| *ms <= MAX_CONFIRM_MS)
        {
            let ms = i64::try_from(ms).unwrap_or(0);
            let mut confirm = span(
                "confirm",
                "wait",
                "confirmation",
                (Timestamp(requested.0 - ms), requested),
            );
            confirm.outcome = Some("confirmed".to_owned());
            confirm.detail = Some(serde_json::json!({ "measured_by": "page" }).to_string());
            self.record_span(confirm).await;
        }
        self.record_span(span(
            "prepare",
            "prepare",
            "checks",
            (requested, run.started_at),
        ))
        .await;
        if let Some(window) = promoted {
            self.record_span(span("promote", "promote", "promotion", window))
                .await;
        }
    }

    /// The commit GitHub reports as `branch`'s head, taken from the sandbox clone into the CD
    /// mirror with every object checked (review C1): what a deploy runs never depends on
    /// anything an agent can write.
    async fn cd_verified_head(
        &self,
        sandbox: &SandboxRecord,
        branch: &str,
        token: &Secret,
    ) -> AppResult<String> {
        let sha = self
            .deps
            .github
            .branch_sha(token, &sandbox.repo_owner, &sandbox.repo_name, branch)
            .await?
            .ok_or_else(|| {
                AppError::Conflict(format!("branch '{branch}' does not exist on GitHub"))
            })?;
        self.deps
            .git
            .cd_mirror_fetch(
                &self.cd_mirror(&sandbox.id),
                &self.sandbox_repo(&sandbox.id),
                branch,
                &sha,
            )
            .await
            .map_err(|err| {
                AppError::Conflict(format!(
                    "nothing ran: the commit GitHub reports for '{branch}' ({}) could not be \
                     taken as it is ({}); deploy again in a minute",
                    sha.get(..7).unwrap_or(&sha),
                    err.public_message()
                ))
            })?;
        Ok(sha)
    }

    /// The CI the integration branch defines, read where no job or agent can write (SPEC-142,
    /// review B1): the integration head GitHub reports, taken from the sandbox clone into the CD
    /// mirror with every object checked and replacement refs ignored. What CI jobs may use of the
    /// agent secrets comes from here, never from the sandbox clone they can write.
    pub(crate) async fn ci_verified_gate(
        &self,
        sandbox: &SandboxRecord,
    ) -> AppResult<Option<yulius_domain::ci::Gate>> {
        let mirror_lock = mirror_lock(&sandbox.id);
        let _mirror = mirror_lock.lock().await;
        let profile = delivery_profile(sandbox);
        let token = self.dlv_installation_token(sandbox).await?;
        let branch = &profile.integration_branch;
        // The clone usually has the head already; it is fetched only when it does not, so a
        // concurrent fetch or a network blip does not withhold a grant (review round 2).
        let head = match self.cd_verified_head(sandbox, branch, &token).await {
            Ok(head) => head,
            Err(_) => {
                self.dlv_refresh_clone(sandbox).await?;
                self.cd_verified_head(sandbox, branch, &token).await?
            }
        };
        let jobs = self
            .ci_jobs_at(&self.cd_mirror(&sandbox.id), &head)
            .await
            .map_err(AppError::Conflict)?;
        let gate = yulius_domain::ci::effective_gate(
            crate::delivery::profile_gate(sandbox).as_deref(),
            &jobs,
        );
        Ok(self.ci_with_security(sandbox, gate).await)
    }

    /// `[cd.*]` of the verified commit `sha` in the CD mirror.
    async fn cd_verified_config(&self, sandbox: &SandboxRecord, sha: &str) -> AppResult<CdConfig> {
        self.cd_config_at(&self.cd_mirror(&sandbox.id), sha)
            .await
            .map_err(|problem| {
                AppError::Conflict(format!(
                    "CD can't be read at {}: {problem}",
                    sha.get(..7).unwrap_or(sha)
                ))
            })
    }

    /// Everything a deploy needs, from verified sources (CDC-003): the environment's stage, the
    /// commit, its table, the secrets, and for production with `promote` the promotion, after
    /// which the new production head is read and authorized again (review M2). `resolved` gets
    /// the stage as soon as it is known. A named environment's stage is known only once the
    /// verified heads are read, so an agent's request for a production-stage one is refused after
    /// that reading, before any secret or promotion (review L2).
    async fn cd_prepare(
        &self,
        sandbox: &SandboxRecord,
        env: &str,
        requester: Requester<'_>,
        actor: &CdActor<'_>,
        expected: Option<&str>,
        resolved: &mut Option<CdStage>,
    ) -> AppResult<Prepared> {
        // One deploy at a time reads and writes a sandbox's mirror (round 2, L-b).
        let mirror_lock = mirror_lock(&sandbox.id);
        let _mirror = mirror_lock.lock().await;
        let profile = delivery_profile(sandbox);
        self.dlv_refresh_clone(sandbox).await?;
        let token = self.dlv_installation_token(sandbox).await?;
        let integration = self
            .cd_verified_head(sandbox, &profile.integration_branch, &token)
            .await?;
        let integration_config = self.cd_verified_config(sandbox, &integration).await;
        // The stage of a name (SPEC-257 CDN-004): the integration head's first, else the
        // production head's.
        let mut stage = dom::resolve_stage(env, integration_config.as_ref().ok(), None);
        if stage.is_none() {
            let production = self
                .cd_verified_head(sandbox, &profile.production_branch, &token)
                .await?;
            let config = self.cd_verified_config(sandbox, &production).await.ok();
            stage = dom::resolve_stage(env, None, config.as_ref());
        }
        let Some(stage) = stage else {
            return Err(refusal(&CdRefusal::not_defined(env)));
        };
        *resolved = Some(stage);
        dom::may_deploy(stage, requester, &sandbox.slug).map_err(|r| refusal(&r))?;
        // Production without promotion does not depend on dev's table being valid (round 2, L-a).
        let integration_config = match integration_config {
            Ok(config) => config,
            Err(_) if stage == CdStage::Prod => CdConfig::default(),
            Err(err) => return Err(err),
        };
        let promotes = stage == CdStage::Prod
            && integration_config
                .target_of(env, CdStage::Prod)
                .is_some_and(|prod| prod.promote);
        let (branch, sha, config) = match stage {
            CdStage::Dev => (
                profile.integration_branch.clone(),
                integration,
                integration_config,
            ),
            CdStage::Prod if promotes => (
                profile.production_branch.clone(),
                integration,
                integration_config,
            ),
            CdStage::Prod => {
                let production = self
                    .cd_verified_head(sandbox, &profile.production_branch, &token)
                    .await?;
                // Only the integration head decides a promotion (review L4).
                let config = no_promotion(self.cd_verified_config(sandbox, &production).await?);
                (profile.production_branch.clone(), production, config)
            }
        };
        // The person confirmed what they were shown (round 2, M-C): the commit it deploys, or
        // for a promotion the integration commit it promotes.
        if let Some(expected) = expected.map(str::trim).filter(|e| !e.is_empty())
            && !(expected.len() >= 7 && sha.starts_with(&expected.to_ascii_lowercase()))
        {
            return Err(AppError::Conflict(format!(
                "nothing ran: {} moved since you looked (it is now {}); review it and deploy again",
                if promotes {
                    &profile.integration_branch
                } else {
                    &branch
                },
                sha.get(..7).unwrap_or(&sha)
            )));
        }
        dom::authorize(&config, env, stage, requester, &sandbox.slug).map_err(|r| refusal(&r))?;
        let Some(target) = config.target_of(env, stage).cloned() else {
            return Err(AppError::Internal(
                "an authorized environment has a table".into(),
            ));
        };
        if self
            .deps
            .store
            .cd_runs(&sandbox.id, CD_RUN_LIST_LIMIT)
            .await?
            .iter()
            .any(|run| run.environment == env && run.state == RunState::Running)
        {
            return Err(running_refusal(env));
        }
        let asker = match actor {
            CdActor::Agent(ctx) => Some(*ctx),
            CdActor::Person(..) => None,
        };
        let why = format!("CD {} deploy", env_label(env));
        self.cd_names_present(sandbox, &target).await?;
        // Secrets first: a refusal never comes after a promotion.
        let secrets = self
            .cd_secret_values(sandbox, stage, &target.secrets, asker, &why)
            .await?;
        if !(stage == CdStage::Prod && target.promote) {
            return Ok(Prepared {
                stage,
                branch,
                sha,
                target,
                secrets,
                note: None,
                promoted: None,
            });
        }
        let CdActor::Person(user) = actor else {
            return Err(refusal(&CdRefusal::ProductionIsAPersons));
        };
        let mirror = self.cd_mirror(&sandbox.id);
        let before = self
            .cd_verified_head(sandbox, &profile.production_branch, &token)
            .await?;
        // From verified commits, never the agents' clone (round 2, M-C).
        let contained = self.deps.git.is_ancestor(&mirror, &sha, &before).await?;
        let mut promoted = None;
        let note = if contained {
            format!(
                "'{}' already contains '{}': nothing to promote",
                profile.production_branch, profile.integration_branch
            )
        } else {
            let promoting = self.now();
            let note = self.cd_promote(sandbox, &profile, user, &sha).await?;
            promoted = Some((promoting, self.now()));
            note
        };
        self.dlv_refresh_clone(sandbox).await?;
        let production = self
            .cd_verified_head(sandbox, &profile.production_branch, &token)
            .await?;
        if !self
            .deps
            .git
            .is_ancestor(&mirror, &sha, &production)
            .await?
        {
            return Err(AppError::Conflict(format!(
                "nothing was deployed: '{}' at {} does not contain the promoted {} ({note})",
                profile.production_branch,
                production.get(..7).unwrap_or(&production),
                sha.get(..7).unwrap_or(&sha)
            )));
        }
        // What production holds now is what runs, with its own table.
        let deployed = self.cd_verified_config(sandbox, &production).await?;
        let mut checked = no_promotion(deployed);
        if let Some(prod) = checked
            .environments
            .iter_mut()
            .find(|target| target.name == env && target.stage == CdStage::Prod)
            && prod.deploy.is_none()
            && prod.verify.is_none()
        {
            // Promotion alone was the deploy: the run records it.
            prod.promote = true;
        }
        dom::authorize(&checked, env, stage, requester, &sandbox.slug).map_err(|r| refusal(&r))?;
        let Some(target) = checked.target_of(env, stage).cloned() else {
            return Err(refusal(&CdRefusal::not_defined(env)));
        };
        self.cd_names_present(sandbox, &target).await?;
        let secrets = self
            .cd_secret_values(sandbox, stage, &target.secrets, None, &why)
            .await?;
        Ok(Prepared {
            stage,
            branch,
            sha: production,
            target,
            secrets,
            note: Some(note),
            promoted,
        })
    }

    /// Promotes the integration branch for a person's production deploy with the profile's
    /// promotion; with a merge-commit promotion the pull request is merged at once when its
    /// checks passed. Returns the note the run keeps.
    async fn cd_promote(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        user: &UserRecord,
        confirmed: &str,
    ) -> AppResult<String> {
        let (integration, production) = (&profile.integration_branch, &profile.production_branch);
        let token = self
            .person_github_token(user, &format!("Deploying {production}"))
            .await?;
        let mut result = self
            .cd_promote_once(sandbox, profile, user, &token, confirmed)
            .await?;
        if result.result == "pr_opened" {
            let url = result.sha.clone().unwrap_or_default();
            result = self
                .cd_promote_once(sandbox, profile, user, &token, confirmed)
                .await
                .map_err(|err| {
                    AppError::Conflict(format!(
                        "nothing was deployed: the promotion pull request {url} is open and cannot \
                         merge yet ({}); deploy production again once it can",
                        err.public_message()
                    ))
                })?;
        }
        Ok(format!(
            "promoted '{integration}' to '{production}' at {}",
            result.sha.as_deref().unwrap_or("?")
        ))
    }

    async fn cd_promote_once(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        user: &UserRecord,
        token: &Secret,
        confirmed: &str,
    ) -> AppResult<yulius_contracts::api::git::PromoteResult> {
        self.dlv_promote_exactly(
            sandbox,
            profile,
            yulius_domain::delivery::Initiator::User,
            user,
            token,
            true,
            Some(confirmed),
        )
        .await
    }

    /// Background part of a run: its files, the commands, the record of the end.
    async fn cd_execute(
        self: Arc<Self>,
        sandbox: SandboxRecord,
        run: CdRunRecord,
        secrets: Vec<(String, String)>,
        (states, named): (Vec<String>, Vec<String>),
        mut cancel: watch::Receiver<Option<String>>,
    ) {
        let mut ring = OutputRing::new(OUTPUT_BYTES);
        if let Some(note) = &run.note {
            ring.push_str(&format!("[yulius] {note}\n"));
        }
        ring.push_str(&format!(
            "[yulius] deploying {} at {} to {}\n",
            run.branch,
            run.sha,
            env_label(&run.environment)
        ));
        let run_dir = self.cd_run_dir(&run.id);
        let mut held = CdHeld::default();
        let (ended, usage) = match self
            .cd_process(
                &sandbox,
                &run,
                RunInputs {
                    secrets,
                    states: &states,
                    variables: &named,
                    held: &mut held,
                },
                &run_dir,
                &mut ring,
                &mut cancel,
            )
            .await
        {
            Ok((ended, usage)) => (Ok(ended), usage),
            Err(reason) => (Err(reason), None),
        };
        // Saved after the deploy and the verify, whatever their outcome; then released.
        let unsaved = self
            .cd_save_states(&sandbox, &run, &run_dir, &held.restored, &mut ring)
            .await;
        drop(held);
        // The run's files (the tree, its HOME, its log) go with it.
        let _ = std::fs::remove_dir_all(&run_dir);
        let (state, exit_code, reason) = match ended {
            Ok(Ended::Exit(exit)) if exit.spawn_failed => (
                RunState::Error,
                None,
                Some("the run could not be started".to_owned()),
            ),
            Ok(Ended::Exit(exit)) if exit.is_clean() => (RunState::Passed, Some(0), None),
            Ok(Ended::Exit(exit)) => (
                RunState::Failed,
                exit.code.or_else(|| exit.signal.map(|signal| 128 + signal)),
                None,
            ),
            Ok(Ended::Cancelled(reason)) => (RunState::Cancelled, None, Some(reason)),
            Ok(Ended::TimedOut) => (
                RunState::Error,
                None,
                Some(format!(
                    "timed out after {}",
                    yulius_domain::ci::format_duration(
                        i64::try_from(self.config.ci.job_timeout.as_millis()).unwrap_or(i64::MAX)
                    )
                )),
            ),
            Ok(Ended::OutputLimit) => (
                RunState::Error,
                None,
                Some("the run wrote more than 256 MiB of output".to_owned()),
            ),
            Ok(Ended::Lost) => (
                RunState::Error,
                None,
                Some("the run's process host was lost".to_owned()),
            ),
            Ok(Ended::NoDisk(reason)) | Err(reason) => (RunState::Error, None, Some(reason)),
        };
        if let Some(reason) = &reason {
            ring.push_str(&format!("\n[yulius] {reason}\n"));
        }
        // A state the run could not save is lost work: the run is an error, whatever it did,
        // and says what it did (review of SPEC-278, m6).
        let (state, reason) = match unsaved {
            Some(unsaved) if state == RunState::Passed => (
                RunState::Error,
                Some(format!("the deploy passed, but {unsaved}")),
            ),
            Some(unsaved) => (
                RunState::Error,
                Some(format!(
                    "{}; and {unsaved}",
                    reason.unwrap_or_else(|| format!("the run {}", state.as_str()))
                )),
            ),
            None => (state, reason),
        };
        let output = self
            .redact_for_sandbox(&sandbox.id, &whole_lines(&ring))
            .await;
        let end = CdRunEnd {
            state,
            reason,
            exit_code,
            output,
            ended_at: self.now(),
            usage,
        };
        if let Err(err) = self.deps.store.finish_cd_run(&run.id, &end).await {
            tracing::error!(error = %err, run = %run.id, "recording a CD run's end failed");
        }
        if let Ok(mut executing) = self.ci.executing.lock() {
            executing.remove(&run.id);
        }
        self.dlv_audit(
            &run.requested_by,
            "cd.finished",
            &format!("{}/{}", sandbox.slug, run.environment),
            &format!(
                "run {}: {} (exit {:?}) at {}",
                run.id,
                state.as_str(),
                exit_code,
                run.sha
            ),
        )
        .await;
    }

    /// The run's containment (review C1, H-A). Dev joins the sandbox: agents build and use
    /// dev, with their tools. Production joins only the sandbox's network (no host loopback),
    /// on the host's read-only system with empty `/var`, `/root` and `/home` and the sandbox's
    /// resolver: nothing an agent installed or wrote runs with production's secrets. Without
    /// sandbox isolation both run in the previous containment, production with the run's
    /// directory as its root. Only the run's directory and its environment's cache are bound in.
    pub(super) async fn cd_containment(
        &self,
        sandbox: &SandboxRecord,
        stage: CdStage,
        work: &Path,
        home: &Path,
        cache: &Path,
    ) -> Result<(Containment, Option<crate::isolation::IsolationLease>), String> {
        if !self.config.contained {
            return Ok((Containment::None, None));
        }
        // Held until the run ends: the sandbox's holder is not stopped as idle meanwhile.
        let (join, lease) = match self.isolation_join(sandbox).await {
            Ok(joined) => joined.unzip(),
            Err(err) => return Err(err.public_message()),
        };
        let readonly = self.config.contained_readonly.clone();
        let containment = match (stage, join) {
            (CdStage::Prod, Some(join)) => {
                let resolver = work.join("resolv.conf");
                std::fs::write(&resolver, SANDBOX_RESOLVER)
                    .map_err(|e| format!("writing the run's resolver failed: {e}"))?;
                production_containment(work, home, cache, readonly, &resolver, join)
            }
            (CdStage::Prod, None) => Containment::Bubblewrap {
                sandbox_root: work.to_path_buf(),
                home: home.to_path_buf(),
                writable: vec![cache.to_path_buf()],
                readonly,
                binds: Vec::new(),
                identity: None,
                join: None,
            },
            (CdStage::Dev, join) => Containment::Bubblewrap {
                sandbox_root: self.sandbox_dir(&sandbox.id),
                home: home.to_path_buf(),
                writable: vec![work.to_path_buf(), cache.to_path_buf()],
                readonly: readonly
                    .into_iter()
                    .chain(join.iter().flat_map(|j| j.extra_readonly.iter().cloned()))
                    .collect(),
                binds: Vec::new(),
                identity: None,
                join: join.map(|join| Box::new(ci_join(join))),
            },
        };
        Ok((containment, lease))
    }

    #[allow(clippy::too_many_arguments)] // One run's whole context.
    async fn cd_process(
        &self,
        sandbox: &SandboxRecord,
        run: &CdRunRecord,
        RunInputs {
            secrets,
            states,
            variables: named,
            held,
        }: RunInputs<'_>,
        run_dir: &Path,
        ring: &mut OutputRing,
        cancel: &mut watch::Receiver<Option<String>>,
    ) -> Result<(Ended, super::exec::Used), String> {
        let work = run_dir.join("work");
        let (src, home, log) = (work.join("src"), work.join("home"), work.join("log"));
        let cache = self
            .cd_private_dir(&sandbox.id)
            .join("cache")
            .join(cache_name(&run.environment, run.stage));
        let _ = std::fs::remove_dir_all(run_dir);
        for dir in [&home, &cache] {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("preparing the run's directories failed: {e}"))?;
        }
        self.deps
            .git
            .export_tree(&self.cd_mirror(&sandbox.id), &run.sha, &src)
            .await
            .map_err(|err| {
                format!(
                    "writing {}'s files failed: {}",
                    run.sha,
                    err.public_message()
                )
            })?;
        std::fs::File::create(&log).map_err(|e| format!("creating the run log failed: {e}"))?;
        // The states it names, held first (SPEC-278 CDS-003): another run's wait ends here.
        if !states.is_empty() {
            match self
                .cd_hold_states(sandbox, run, states, ring, cancel)
                .await
            {
                Ok(hold) => held.hold = Some(hold),
                Err(ended) => return Ok((ended, None)),
            }
        }
        let mut state_env = Vec::new();
        let writer = CdWriter {
            environment: &run.environment,
            run: &run.id,
            requested_by: &run.requested_by,
        };
        for name in states {
            let restored = self
                .cd_restore_state(&sandbox.id, name, run_dir, &writer)
                .await
                .map_err(|err| {
                    format!(
                        "nothing ran: restoring the agent state {name} failed: {}",
                        err.public_message()
                    )
                })?;
            let variable = dom::state_variable(name);
            ring.push_str(&format!(
                "[yulius] agent state {name} version {} restored into ${variable}\n",
                restored.version
            ));
            state_env.push((variable, restored.path.to_string_lossy().into_owned()));
            held.restored.push(restored);
        }
        // The project's variables first: Yulius's own and the secrets win (SPEC-278 CDS-007).
        // Production gets only those its verified table names: a person with write access sets
        // them, an administrator deploys production (review of SPEC-278, M3).
        let mut env = Vec::new();
        let mut left_out = Vec::new();
        let variables = self.variables_env(&sandbox.id).await.map_err(|err| {
            format!(
                "reading the project's variables failed: {}",
                err.public_message()
            )
        })?;
        for (name, value) in variables {
            if run.stage == CdStage::Prod && !named.contains(&name) {
                left_out.push(name);
            } else {
                env.push((name, value));
            }
        }
        if !left_out.is_empty() {
            ring.push_str(&format!(
                "[yulius] production runs get only the variables [cd.{}] names; left out: {}\n",
                run.environment,
                left_out.join(", ")
            ));
        }
        let (containment, _lease) = self
            .cd_containment(sandbox, run.stage, &work, &home, &cache)
            .await?;
        let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
        env.extend([
            ("HOME".to_owned(), home.to_string_lossy().into_owned()),
            ("PATH".to_owned(), path),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("TERM".to_owned(), "dumb".to_owned()),
            ("NO_COLOR".to_owned(), "1".to_owned()),
            ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
            ("YULIUS_CD".to_owned(), "1".to_owned()),
            ("YULIUS_CD_ENV".to_owned(), run.environment.clone()),
            ("YULIUS_CD_STAGE".to_owned(), run.stage.as_str().to_owned()),
            ("YULIUS_CD_RUN".to_owned(), run.id.clone()),
            ("YULIUS_CD_SHA".to_owned(), run.sha.clone()),
            ("YULIUS_CD_BRANCH".to_owned(), run.branch.clone()),
            (
                "YULIUS_CD_CACHE".to_owned(),
                cache.to_string_lossy().into_owned(),
            ),
            ("YULIUS_SANDBOX".to_owned(), sandbox.slug.clone()),
        ]);
        env.extend(state_env);
        let plan = LaunchPlan {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                CD_WRAPPER.into(),
                "yulius-cd".into(),
                log.to_string_lossy().into_owned(),
                run.deploy.clone().unwrap_or_default(),
                run.verify.clone().unwrap_or_default(),
            ],
            cwd: src,
            env,
            secret_env: secrets,
            files: Vec::new(),
            containment,
        };
        let token = self.deps.crypto.random_bytes();
        let group = format!("cd-{}", run.id);
        // Its deploy and verify, and the stages their output names (SPEC-137 STG-301).
        let mut stages =
            crate::stages::OutputStages::cd_run(sandbox.id.clone(), &run.id, &run.environment);
        self.ci_watch(
            run_dir,
            (&run.id, &group),
            Sink::CdRun(&run.id, &sandbox.id),
            &plan,
            &token,
            &log,
            (ring, Some(&mut stages), None),
            cancel,
        )
        .await
    }

    async fn cd_require_run(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        run_id: &str,
        action: Action,
    ) -> AppResult<(SandboxRecord, AccessLevel, CdRunRecord)> {
        let sid = SandboxId::parse(sandbox_id).map_err(|_| AppError::NotFound)?;
        let (sandbox, level) = self.require(&ctx.user, &sid, action).await?;
        let run = self
            .deps
            .store
            .cd_run(run_id, true)
            .await?
            .filter(|run| run.sandbox_id == sandbox.id)
            .ok_or(AppError::NotFound)?;
        Ok((sandbox, level, run))
    }

    /// `GET /api/sandboxes/{sid}/cd/runs/{rid}` (read): the run with its output.
    ///
    /// # Errors
    /// `NotFound` for unknown runs or other sandboxes' runs.
    pub async fn cd_run_detail(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        run_id: &str,
    ) -> AppResult<CdRun> {
        let (_, _, run) = self
            .cd_require_run(ctx, sandbox_id, run_id, Action::View)
            .await?;
        Ok(self.cd_run_view(&run, true).await)
    }

    /// A run with no executor: its process host (which outlives a restart) is terminated and
    /// its files removed (review H2), then it ends as `state`.
    async fn cd_end_lost(
        &self,
        run: CdRunRecord,
        state: RunState,
        reason: &str,
    ) -> AppResult<bool> {
        let run_dir = self.cd_run_dir(&run.id);
        if run_dir.exists() {
            let _ = self.deps.session_host.terminate(&run_dir);
            let _ = std::fs::remove_dir_all(&run_dir);
        }
        let full = self.deps.store.cd_run(&run.id, true).await?.unwrap_or(run);
        self.deps
            .store
            .finish_cd_run(
                &full.id,
                &CdRunEnd {
                    state,
                    reason: Some(reason.to_owned()),
                    exit_code: None,
                    output: format!("{}\n[yulius] {reason}\n", full.output),
                    ended_at: self.now(),
                    usage: None,
                },
            )
            .await
    }

    /// `POST /api/sandboxes/{sid}/cd/runs/{rid}/cancel` (write; a production run: administrator).
    /// Kills the run's process group; a run without an executor (lost) is ended as cancelled.
    ///
    /// # Errors
    /// `NotFound`, `Forbidden` below the access, `Conflict` when it already ended.
    pub async fn cd_cancel(
        &self,
        ctx: &AuthContext,
        sandbox_id: &str,
        run_id: &str,
    ) -> AppResult<CdRun> {
        let (sandbox, level, run) = self
            .cd_require_run(ctx, sandbox_id, run_id, Action::Work)
            .await?;
        if run.stage == CdStage::Prod && !level.allows(Action::Administer) {
            return Err(refusal(&CdRefusal::ProductionNeedsAdministrator));
        }
        if run.state != RunState::Running {
            return Err(AppError::Conflict("the run already ended".into()));
        }
        let login = Self::dlv_actor_login(&ctx.user);
        let reason = format!("cancelled by {login}");
        let signalled = self
            .ci
            .executing
            .lock()
            .ok()
            .and_then(|map| {
                map.get(&run.id)
                    .map(|tx| tx.send(Some(reason.clone())).is_ok())
            })
            .unwrap_or(false);
        if !signalled {
            self.cd_end_lost(run.clone(), RunState::Cancelled, &reason)
                .await?;
        }
        self.dlv_audit(
            &login,
            "cd.cancel",
            &format!("{}/{}", sandbox.slug, run.environment),
            &format!("run {}", run.id),
        )
        .await;
        let after = self.deps.store.cd_run(&run.id, true).await?.unwrap_or(run);
        Ok(self.cd_run_view(&after, true).await)
    }

    /// Startup: runs recorded as running have no executor after a restart. Their process hosts
    /// are terminated and they are recorded as `error`, never started again by themselves
    /// (CDR-004). Returns how many. Call once before serving.
    ///
    /// # Errors
    /// Store failures.
    pub async fn reconcile_cd_runs(&self) -> AppResult<u32> {
        let mut closed = 0;
        for run in self.deps.store.running_cd_runs().await? {
            if self
                .ci
                .executing
                .lock()
                .is_ok_and(|map| map.contains_key(&run.id))
            {
                continue;
            }
            let reason = "Yulius restarted while it ran: it was stopped, its outcome is unknown, \
                          and it is not run again by itself";
            if self.cd_end_lost(run, RunState::Error, reason).await? {
                closed += 1;
            }
        }
        Ok(closed)
    }

    /// Deletes old runs (with the CI's retention pass).
    pub(super) async fn cd_prune(&self) {
        let before = Timestamp(self.now().0 - CD_RETAIN_MS);
        if let Err(err) = self.deps.store.prune_cd_runs(CD_KEEP_RUNS, before).await {
            tracing::warn!(error = %err, "pruning CD runs failed");
        }
    }

    /// The latest CD run `task`'s agent started (`yulius wait`, CDT-003).
    pub(crate) async fn cd_latest_run_of_task(
        &self,
        task: &crate::model::TaskRecord,
    ) -> AppResult<Option<CdRunRecord>> {
        self.deps.store.latest_cd_run_of_task(&task.id).await
    }
}

/// What a run's process gets beside its record: the secrets' values, the agent states and project
/// variables its table names, and where the held states go (SPEC-278).
struct RunInputs<'a> {
    secrets: Vec<(String, String)>,
    states: &'a [String],
    variables: &'a [String],
    held: &'a mut CdHeld,
}

/// The agent states a run holds and what it restored of them (SPEC-278).
#[derive(Default)]
struct CdHeld {
    hold: Option<StateHold>,
    restored: Vec<CdRestored>,
}

impl App {
    /// Refuses a deploy naming a project variable the project lacks or an agent state its vault
    /// does not hold (SPEC-278 CDS-002), before anything ran.
    async fn cd_names_present(&self, sandbox: &SandboxRecord, target: &CdTarget) -> AppResult<()> {
        if !target.variables.is_empty() {
            let set = self.variables_env(&sandbox.id).await?;
            if let Some(name) = target
                .variables
                .iter()
                .find(|name| !set.iter().any(|(set, _)| set == *name))
            {
                return Err(AppError::Conflict(format!(
                    "nothing ran: [cd.{}] names the project variable {name}, which this project \
                     does not have; a person sets it on the Secrets page's Variables tab or with \
                     `yulius vars set {name}=…`",
                    target.name
                )));
            }
        }
        if target.states.is_empty() {
            return Ok(());
        }
        let stored = self.deps.store.agent_states(&sandbox.id).await?;
        for name in &target.states {
            if !stored.iter().any(|state| &state.name == name) {
                return Err(AppError::Conflict(format!(
                    "nothing ran: [cd.{}] names the agent state {name}, which this project's \
                     vault does not hold; a person sends it with `yulius secrets import --to \
                     agent --state {name}=DIR`",
                    target.name
                )));
            }
        }
        Ok(())
    }

    /// Holds every state `names` for `run`, waiting while another run holds one (SPEC-278
    /// CDS-003): bounded by the run's timeout, ended by its cancel.
    async fn cd_hold_states(
        &self,
        sandbox: &SandboxRecord,
        run: &CdRunRecord,
        names: &[String],
        ring: &mut OutputRing,
        cancel: &mut watch::Receiver<Option<String>>,
    ) -> Result<StateHold, Ended> {
        let holder = format!("{} run {}", run.environment, run.id);
        let holds = Arc::clone(&self.vault.holds);
        let wait = holds.hold(sandbox.id.as_str(), names, &holder, |elsewhere| {
            ring.push_str(&format!(
                "[yulius] waiting for the agent state {}: {} holds it\n",
                elsewhere.name, elsewhere.holder
            ));
        });
        tokio::select! {
            held = tokio::time::timeout(self.config.ci.job_timeout, wait) => {
                held.map_err(|_| Ended::TimedOut)
            }
            reason = super::exec::wait_cancel(cancel) => Err(Ended::Cancelled(reason)),
        }
    }

    /// Saves each state a run restored (SPEC-278 CDS-004) and says so in its output; the reason
    /// the run is an error when one could not be saved.
    async fn cd_save_states(
        &self,
        sandbox: &SandboxRecord,
        run: &CdRunRecord,
        run_dir: &Path,
        restored: &[CdRestored],
        ring: &mut OutputRing,
    ) -> Option<String> {
        let writer = CdWriter {
            environment: &run.environment,
            run: &run.id,
            requested_by: &run.requested_by,
        };
        let mut unsaved = Vec::new();
        for state in restored {
            match self
                .cd_save_state(&sandbox.id, state, run_dir, &writer)
                .await
            {
                Ok(None) => ring.push_str(&format!(
                    "[yulius] agent state {} unchanged: version {} kept\n",
                    state.name, state.version
                )),
                Ok(Some((version, skipped))) => {
                    ring.push_str(&format!(
                        "[yulius] agent state {} saved as version {version}\n",
                        state.name
                    ));
                    if !skipped.is_empty() {
                        ring.push_str(&format!(
                            "[yulius] links and special files are not kept in a state; left out: \
                             {}\n",
                            skipped.join(", ")
                        ));
                    }
                }
                Err(err) => {
                    let why = err.public_message();
                    ring.push_str(&format!(
                        "[yulius] saving the agent state {} failed: {why}; what this run wrote to \
                         it is not in the vault\n",
                        state.name
                    ));
                    unsaved.push(format!("{} ({why})", state.name));
                }
            }
        }
        (!unsaved.is_empty())
            .then(|| format!("the agent state could not be saved: {}", unsaved.join("; ")))
    }
}

/// One lock per sandbox around its CD mirror's reads and writes.
fn mirror_lock(sandbox: &SandboxId) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    let locks = LOCKS.get_or_init(Default::default);
    let mut map = locks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    map.entry(sandbox.as_str().to_owned()).or_default().clone()
}

/// The resolver of a sandbox's network (`slirp4netns`'s DNS; `yulius_sandbox::base`).
const SANDBOX_RESOLVER: &str = "nameserver 10.0.2.3\n";

/// A production run (review H-A, C1; round 3): it joins its sandbox's holder for the network
/// only (no host loopback); the process host gives it the host's system read-only instead of the
/// sandbox's system layer (`host_system`), with the sandbox's resolver, and only the run's
/// directory and its cache are bound in.
#[must_use]
pub fn production_containment(
    work: &Path,
    home: &Path,
    cache: &Path,
    readonly: Vec<PathBuf>,
    resolver: &Path,
    join: crate::ports::isolation::SandboxJoin,
) -> Containment {
    let mut join = ci_join(join);
    join.host_system = Some(resolver.to_path_buf());
    join.docker_dir = None;
    join.extra_readonly.clear();
    Containment::Bubblewrap {
        sandbox_root: work.to_path_buf(),
        home: home.to_path_buf(),
        writable: vec![cache.to_path_buf()],
        readonly,
        binds: Vec::new(),
        identity: None,
        join: Some(Box::new(join)),
    }
}

/// A production table read from the production head: only the integration head decides a
/// promotion (review L4).
fn no_promotion(mut config: CdConfig) -> CdConfig {
    for target in &mut config.environments {
        target.promote = false;
    }
    config
}

/// The cache directory of the environment `name` of `stage` (review H1): a name relabelled to
/// the other stage gets another cache, so what a dev-stage run leaves there never runs with
/// production's trust. `dev` and `prod` keep theirs (a name has no `.`).
fn cache_name(name: &str, stage: CdStage) -> String {
    if dom::fixed_stage(name) == Some(stage) {
        name.to_owned()
    } else {
        format!("{stage}.{name}")
    }
}

/// What a request naming no environment is told.
const UNKNOWN_ENV: &str = "the environment is a [cd.<name>] of yulius.toml: dev, prod or another \
    name of lowercase letters, digits and '-'";

fn running_refusal(env: &str) -> AppError {
    AppError::Conflict(format!(
        "a deploy to {} is running; wait for it to end (or cancel it)",
        env_label(env)
    ))
}

/// One environment as `cd_status` prints it.
fn environment_text(out: &mut String, env: &CdEnvironment) {
    let state = match env.state.as_str() {
        "ready" => "ready",
        "empty" => "empty (defined, nothing to deploy yet)",
        _ => "not defined",
    };
    out.push_str(&format!(
        "{} ({}): {state}",
        env.label,
        if env.stage == "dev" {
            format!("[cd.{}] from {}", env.name, env.branch)
        } else {
            format!("[cd.{}], deploys {}", env.name, env.branch)
        }
    ));
    if let Some(about) = &env.about {
        out.push_str(&format!(" · {about}"));
    }
    out.push('\n');
    if let Some(problem) = &env.problem {
        out.push_str(&format!("  problem: {problem}\n"));
    }
    if env.promote {
        out.push_str("  promotes the integration branch first (a person's deploy)\n");
    }
    for (what, command) in [("deploy", &env.deploy), ("verify", &env.verify)] {
        if let Some(command) = command {
            out.push_str(&format!("  {what}: {command}\n"));
        }
    }
    if !env.secrets.is_empty() {
        let secrets: Vec<String> = env
            .secrets
            .iter()
            .map(|s| match s.state.as_str() {
                "agents_use" => format!(
                    "{} agents use it (production never gets it: name a separate secret)",
                    s.name
                ),
                state => format!("{} {}", s.name, state.replace('_', " ")),
            })
            .collect();
        out.push_str(&format!("  secrets: {}\n", secrets.join(", ")));
    }
    if !env.states.is_empty() {
        let states: Vec<String> = env
            .states
            .iter()
            .map(|s| match (s.version, &s.held_by) {
                (None, _) => format!(
                    "{} missing (send it with yulius secrets import --state)",
                    s.name
                ),
                (Some(v), Some(by)) => format!("{} version {v}, held by {by}", s.name),
                (Some(v), None) => format!("{} version {v}", s.name),
            })
            .collect();
        out.push_str(&format!(
            "  states (restored into the run, saved back after it): {}\n",
            states.join(", ")
        ));
    }
    if env.stage == "prod" && (env.deploy.is_some() || env.verify.is_some()) {
        out.push_str("  no Docker engine in production runs yet (task #265)\n");
    }
    match (&env.deployed, &env.head) {
        (Some(deployed), Some(head)) if deployed == head => {
            out.push_str(&format!(
                "  deployed: {} (the branch head)\n",
                short_sha(deployed)
            ));
        }
        (Some(deployed), head) => out.push_str(&format!(
            "  deployed: {}; head {}\n",
            short_sha(deployed),
            head.as_deref().map_or("unknown", short_sha)
        )),
        (None, _) => out.push_str("  deployed: never\n"),
    }
    if let Some(run) = &env.last_run {
        out.push_str(&format!("  last run: {}\n", run_line(run)));
    }
}

fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

fn run_line(run: &CdRun) -> String {
    let mut line = format!(
        "{} {} {} at {} by {} ({})",
        run.run_id,
        run.environment,
        run.state,
        short_sha(&run.commit),
        run.requested_by,
        run.started_at
    );
    if let Some(reason) = &run.reason {
        line.push_str(&format!(": {reason}"));
    }
    line
}

impl App {
    /// `cd_status`: the project's CD and recent runs, or one run with the end of its output.
    ///
    /// # Errors
    /// `NotFound` for another project's or an unknown run.
    pub(crate) async fn tool_cd_status(
        &self,
        ctx: &AgentContext,
        args: &serde_json::Value,
    ) -> AppResult<serde_json::Value> {
        let sandbox = self
            .deps
            .store
            .sandbox(&ctx.task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        crate::playground::refuse_in_playground(&sandbox, "CD")?;
        if let Some(id) = args.get("run_id").and_then(serde_json::Value::as_str) {
            let run = self
                .deps
                .store
                .cd_run(id, true)
                .await?
                .filter(|run| run.sandbox_id == sandbox.id)
                .ok_or(AppError::NotFound)?;
            let view = self.cd_run_view(&run, true).await;
            let output = view.output.clone().unwrap_or_default();
            let tail = crate::delivery::tail_bytes(&output, 6 * 1024);
            let mut text = format!("Run {}\n", run_line(&view));
            for (what, command) in [("deploy", &view.deploy), ("verify", &view.verify)] {
                if let Some(command) = command {
                    text.push_str(&format!("{what}: {command}\n"));
                }
            }
            text.push_str(&format!("Output (end):\n{tail}"));
            return Ok(serde_json::json!({"text": text, "run": view}));
        }
        let overview = self.cd_overview_of(&sandbox, AccessLevel::Write).await?;
        let mut text = String::from("CD (deployment):\n");
        for env in &overview.environments {
            environment_text(&mut text, env);
        }
        if overview.runs.is_empty() {
            text.push_str("No runs yet.\n");
        } else {
            text.push_str("Recent runs:\n");
            for run in overview.runs.iter().take(5) {
                text.push_str(&format!("- {}\n", run_line(run)));
            }
        }
        text.push_str(&format!(
            "You may deploy the dev-stage environments (cd_deploy); production-stage ones are deployed by a person on {}/s/{}/ci?section=cd.",
            self.config.public_url, sandbox.slug
        ));
        Ok(serde_json::json!({"text": text, "cd": overview}))
    }

    /// `cd_deploy`: starts a dev run for the task's agent.
    ///
    /// # Errors
    /// `Invalid` arguments; production is `Forbidden`; every refusal of [`App::cd_deploy`].
    pub(crate) async fn tool_cd_deploy(
        self: &Arc<Self>,
        ctx: &AgentContext,
        args: &serde_json::Value,
    ) -> AppResult<serde_json::Value> {
        let name = args
            .get("environment")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| AppError::Invalid("environment is required (dev)".into()))?;
        let env = dom::env_name(name).ok_or_else(|| AppError::Invalid(UNKNOWN_ENV.into()))?;
        let run = self.cd_deploy_for_task(ctx, &env).await?;
        let view = self.cd_run_view(&run, false).await;
        let text = format!(
            "Deploying {}: run {} of {} at {}. Start `yulius wait` in the background: it returns when the run ends; then read it with cd_status {{run_id}}.",
            env_label(&run.environment),
            run.id,
            run.branch,
            short_sha(&run.sha)
        );
        Ok(serde_json::json!({"text": text, "run": view}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::isolation::SandboxJoin;

    /// #704 (SPEC-237): `cd_status` tells an agent what a production secret agents use means.
    #[test]
    fn agents_read_that_production_never_gets_a_secret_agents_use() {
        let env = CdEnvironment {
            name: "prod".into(),
            label: "production".into(),
            stage: "prod".into(),
            state: "ready".into(),
            branch: "main".into(),
            head: None,
            problem: None,
            about: None,
            deploy: Some("sh deploy.sh".into()),
            verify: None,
            promote: false,
            promotes_from: None,
            secrets: vec![
                CdSecret {
                    name: "PAY_KEY".into(),
                    state: "agents_use".into(),
                },
                CdSecret {
                    name: "HOST".into(),
                    state: "not_allowed".into(),
                },
            ],
            states: Vec::new(),
            deployed: None,
            last_run: None,
        };
        let mut text = String::new();
        environment_text(&mut text, &env);
        assert!(
            text.contains(
                "secrets: PAY_KEY agents use it (production never gets it: name a separate secret), \
                 HOST not allowed"
            ),
            "{text}"
        );
    }

    /// Review H-A and C1: production joins the sandbox's network only. Its system is the host's,
    /// read-only, `/var` (Yulius's data) is empty, the resolver is the sandbox's, and nothing of
    /// the sandbox (its directory, its system layer) is bound in.
    #[test]
    fn production_runs_on_the_sandbox_network_with_the_hosts_system() {
        let join = SandboxJoin {
            holder_pid: 7,
            user_ns: 1,
            net_ns: 2,
            mnt_ns: 3,
            cgroup: Some("/cg/agents".into()),
            ci_cgroup: Some("/cg/ci".into()),
            system_root: "/data/sandboxes/s1/system/root".into(),
            hostname: "app".into(),
            exec_shim: None,
            extra_readonly: vec!["/opt/claude".into()],
            private_dirs: vec!["/tmp/x".into()],
            system_passwd: None,
            system_group: None,
            host_system: None,
            docker_dir: None,
        };
        let (work, home, cache) = (
            Path::new("/data/runs/cd-r/work"),
            Path::new("/data/runs/cd-r/work/home"),
            Path::new("/data/cd/s1/cache/prod"),
        );
        let resolver = Path::new("/data/runs/cd-r/work/resolv.conf");
        let Containment::Bubblewrap {
            sandbox_root,
            writable,
            readonly,
            binds,
            join: Some(joined),
            ..
        } = production_containment(
            work,
            home,
            cache,
            vec!["/usr/local/bin/yulius".into()],
            resolver,
            join,
        )
        else {
            panic!("a joined containment");
        };
        assert_eq!(sandbox_root, work);
        assert_eq!(writable, [cache]);
        assert_eq!(readonly, [PathBuf::from("/usr/local/bin/yulius")]);
        assert_eq!(
            joined.host_system.as_deref(),
            Some(resolver),
            "the host's system, not the layer, with the sandbox's resolver"
        );
        assert!(joined.extra_readonly.is_empty());
        assert_eq!(
            joined.system_root,
            PathBuf::from("/data/sandboxes/s1/system/root"),
            "kept for validation; the process host does not bind it in host mode"
        );
        assert_eq!(joined.net_ns, 2, "the sandbox's network: no host loopback");
        assert_eq!(joined.cgroup.as_deref(), Some(Path::new("/cg/ci")));
        assert!(binds.is_empty());
        let everything = format!("{readonly:?} {writable:?} {binds:?} {sandbox_root:?}");
        assert!(!everything.contains("/data/sandboxes"), "{everything}");
    }
}
