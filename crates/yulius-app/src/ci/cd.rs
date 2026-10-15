//! CD (deployment): runs of a project's dev and production environments (SPEC-134, ADR-0109,
//! ADR-0110).
//!
//! A CD run deploys one environment from its protected branch: Yulius fetches, asks GitHub for
//! the branch head, takes exactly that commit from the sandbox clone into a mirror only Yulius
//! writes (every object checked, replacement refs ignored), reads `[cd.<env>]` from `yulius.toml`
//! there, writes the commit's files into the run's private directory and runs the `deploy`
//! command and then the `verify` command through the process host. Dev reads the integration
//! branch; production reads the commit it deploys (the integration head decides whether it
//! promotes first; after a promotion the new production head is read again).
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
use yulius_contracts::api::cd::{CdDeploy, CdEnvironment, CdOverview, CdRun, CdSecret};
use yulius_domain::cd::{self as dom, CdConfig, CdEnv, CdRefusal, CdState, CdTarget, Requester};
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
    /// Environment.
    pub environment: CdEnv,
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
    branch: String,
    sha: String,
    target: CdTarget,
    secrets: Vec<(String, String)>,
    note: Option<String>,
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
    fn cd_private_dir(&self, sandbox: &SandboxId) -> PathBuf {
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

    /// For the page: where `env` comes from in the clone as last fetched (CDC-003). A deploy
    /// never uses this: it reads the verified mirror ([`App::cd_prepare`]).
    async fn cd_shown_source(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        env: CdEnv,
    ) -> ShownSource {
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
        if env == CdEnv::Dev {
            return ShownSource {
                branch: profile.integration_branch.clone(),
                head: integration,
                config: integration_config,
                promotes_from: None,
            };
        }
        let production = head(profile.production_branch.clone()).await;
        let promotes = integration_config
            .as_ref()
            .is_ok_and(|config| config.prod.as_ref().is_some_and(|prod| prod.promote));
        let promotes_from = promotes.then(|| integration.clone()).flatten();
        let config = if promotes {
            integration_config
        } else {
            match &production {
                Some(sha) => self.cd_config_at(&repo, sha).await.map(no_promotion),
                None => Ok(CdConfig::default()),
            }
        };
        ShownSource {
            branch: profile.production_branch.clone(),
            head: production,
            config,
            promotes_from,
        }
    }

    async fn cd_run_view(&self, run: &CdRunRecord, with_output: bool) -> CdRun {
        CdRun {
            run_id: run.id.clone(),
            environment: run.environment.as_str().to_owned(),
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
        let mut environments = Vec::with_capacity(2);
        for env in CdEnv::ALL {
            let source = self.cd_shown_source(sandbox, &profile, env).await;
            let (target, problem) = match &source.config {
                Ok(config) => (config.target(env).cloned(), None),
                Err(problem) => (None, Some(problem.clone())),
            };
            let state = match &source.config {
                Ok(config) => config.state(env),
                Err(_) => CdState::NotDefined,
            };
            let secrets = match &target {
                Some(target) => self
                    .cd_secret_states(&sandbox.id, env, &target.secrets)
                    .await?
                    .into_iter()
                    .map(|(name, state)| CdSecret {
                        name,
                        state: state.as_str().to_owned(),
                    })
                    .collect(),
                None => Vec::new(),
            };
            let deployed = self
                .deps
                .store
                .last_passed_cd_run(&sandbox.id, env)
                .await?
                .map(|run| run.sha);
            let last_run = match runs.iter().find(|run| run.environment == env) {
                Some(run) => Some(self.cd_run_view(run, false).await),
                None => None,
            };
            let target = target.unwrap_or_default();
            environments.push(CdEnvironment {
                name: env.as_str().to_owned(),
                label: env.label().to_owned(),
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
        let env = CdEnv::parse(environment)
            .ok_or_else(|| AppError::Invalid("the environment is dev or prod".into()))?;
        let requester = Requester::Person {
            administrator: level.allows(Action::Administer),
            confirmation: req.confirm.as_deref(),
        };
        let run = self
            .cd_start(
                &sandbox,
                env,
                requester,
                &CdActor::Person(&ctx.user),
                req.expect.as_deref(),
            )
            .await?;
        Ok(self.cd_run_view(&run, false).await)
    }

    /// A deploy of `env` for a task's agent (`cd_deploy`) under its owner's write access.
    ///
    /// # Errors
    /// As [`App::cd_deploy`]; production is always `Forbidden`.
    pub(crate) async fn cd_deploy_for_task(
        self: &Arc<Self>,
        ctx: &AgentContext,
        env: CdEnv,
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
        self.cd_start(&sandbox, env, Requester::Agent, &CdActor::Agent(ctx), None)
            .await
    }

    /// Checks everything, promotes when production says so, records the run and starts it.
    async fn cd_start(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        env: CdEnv,
        requester: Requester<'_>,
        actor: &CdActor<'_>,
        expected: Option<&str>,
    ) -> AppResult<CdRunRecord> {
        // A playground has no deploy: its Git stays on the server (SPEC-099).
        crate::playground::refuse_in_playground(sandbox, "A deploy")?;
        let target_name = format!("{}/{}", sandbox.slug, env.as_str());
        // Who before anything else: an agent never learns more of production than the refusal.
        if let Err(refused) = dom::may_deploy(env, requester, &sandbox.slug) {
            self.dlv_audit(
                &actor.label(),
                "cd.refused",
                &target_name,
                &refused.to_string(),
            )
            .await;
            return Err(refusal(&refused));
        }
        let prepared = match self
            .cd_prepare(sandbox, env, requester, actor, expected)
            .await
        {
            Ok(prepared) => prepared,
            Err(err) => {
                if env == CdEnv::Prod {
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
            environment: env,
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
        let detail = format!(
            "run {} of {} at {}; deploy: {}; verify: {}; secrets: [{}]{}",
            run.id,
            run.branch,
            run.sha,
            prepared.target.deploy.as_deref().unwrap_or("-"),
            prepared.target.verify.as_deref().unwrap_or("-"),
            prepared.target.secrets.join(" "),
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
            app.cd_execute(sandbox, started, prepared.secrets, rx).await;
        });
        Ok(run)
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

    /// Everything a deploy needs, from verified sources (CDC-003): the commit, its table, the
    /// secrets, and for production with `promote` the promotion, after which the new production
    /// head is read and authorized again (review M2).
    async fn cd_prepare(
        &self,
        sandbox: &SandboxRecord,
        env: CdEnv,
        requester: Requester<'_>,
        actor: &CdActor<'_>,
        expected: Option<&str>,
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
        // Production without promotion does not depend on dev's table being valid (round 2, L-a).
        let integration_config = match self.cd_verified_config(sandbox, &integration).await {
            Ok(config) => config,
            Err(_) if env == CdEnv::Prod => CdConfig::default(),
            Err(err) => return Err(err),
        };
        let promotes = env == CdEnv::Prod
            && integration_config
                .prod
                .as_ref()
                .is_some_and(|prod| prod.promote);
        let (branch, sha, config) = match env {
            CdEnv::Dev => (
                profile.integration_branch.clone(),
                integration,
                integration_config,
            ),
            CdEnv::Prod if promotes => (
                profile.production_branch.clone(),
                integration,
                integration_config,
            ),
            CdEnv::Prod => {
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
        dom::authorize(&config, env, requester, &sandbox.slug).map_err(|r| refusal(&r))?;
        let Some(target) = config.target(env).cloned() else {
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
        let why = format!("CD {} deploy", env.label());
        // Secrets first: a refusal never comes after a promotion.
        let secrets = self
            .cd_secret_values(sandbox, env, &target.secrets, asker, &why)
            .await?;
        if !(env == CdEnv::Prod && target.promote) {
            return Ok(Prepared {
                branch,
                sha,
                target,
                secrets,
                note: None,
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
        let note = if contained {
            format!(
                "'{}' already contains '{}': nothing to promote",
                profile.production_branch, profile.integration_branch
            )
        } else {
            self.cd_promote(sandbox, &profile, user, &sha).await?
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
        if let Some(prod) = checked.prod.as_mut()
            && prod.deploy.is_none()
            && prod.verify.is_none()
        {
            // Promotion alone was the deploy: the run records it.
            prod.promote = true;
        }
        dom::authorize(&checked, env, requester, &sandbox.slug).map_err(|r| refusal(&r))?;
        let Some(target) = checked.prod.clone() else {
            return Err(refusal(&CdRefusal::NotDefined(env.label(), env.as_str())));
        };
        let secrets = self
            .cd_secret_values(sandbox, env, &target.secrets, None, &why)
            .await?;
        Ok(Prepared {
            branch,
            sha: production,
            target,
            secrets,
            note: Some(note),
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
            run.environment.label()
        ));
        let run_dir = self.cd_run_dir(&run.id);
        let ended = self
            .cd_process(&sandbox, &run, secrets, &run_dir, &mut ring, &mut cancel)
            .await;
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
        let output = self
            .redact_for_sandbox(&sandbox.id, &whole_lines(&ring))
            .await;
        let end = CdRunEnd {
            state,
            reason,
            exit_code,
            output,
            ended_at: self.now(),
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
            &format!("{}/{}", sandbox.slug, run.environment.as_str()),
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
    async fn cd_containment(
        &self,
        sandbox: &SandboxRecord,
        env: CdEnv,
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
        let containment = match (env, join) {
            (CdEnv::Prod, Some(join)) => {
                let resolver = work.join("resolv.conf");
                std::fs::write(&resolver, SANDBOX_RESOLVER)
                    .map_err(|e| format!("writing the run's resolver failed: {e}"))?;
                production_containment(work, home, cache, readonly, &resolver, join)
            }
            (CdEnv::Prod, None) => Containment::Bubblewrap {
                sandbox_root: work.to_path_buf(),
                home: home.to_path_buf(),
                writable: vec![cache.to_path_buf()],
                readonly,
                binds: Vec::new(),
                identity: None,
                join: None,
            },
            (CdEnv::Dev, join) => Containment::Bubblewrap {
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
        secrets: Vec<(String, String)>,
        run_dir: &Path,
        ring: &mut OutputRing,
        cancel: &mut watch::Receiver<Option<String>>,
    ) -> Result<Ended, String> {
        let work = run_dir.join("work");
        let (src, home, log) = (work.join("src"), work.join("home"), work.join("log"));
        let cache = self
            .cd_private_dir(&sandbox.id)
            .join("cache")
            .join(run.environment.as_str());
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
        let (containment, _lease) = self
            .cd_containment(sandbox, run.environment, &work, &home, &cache)
            .await?;
        let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
        let env = vec![
            ("HOME".to_owned(), home.to_string_lossy().into_owned()),
            ("PATH".to_owned(), path),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("TERM".to_owned(), "dumb".to_owned()),
            ("NO_COLOR".to_owned(), "1".to_owned()),
            ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
            ("YULIUS_CD".to_owned(), "1".to_owned()),
            (
                "YULIUS_CD_ENV".to_owned(),
                run.environment.as_str().to_owned(),
            ),
            ("YULIUS_CD_RUN".to_owned(), run.id.clone()),
            ("YULIUS_CD_SHA".to_owned(), run.sha.clone()),
            ("YULIUS_CD_BRANCH".to_owned(), run.branch.clone()),
            (
                "YULIUS_CD_CACHE".to_owned(),
                cache.to_string_lossy().into_owned(),
            ),
            ("YULIUS_SANDBOX".to_owned(), sandbox.slug.clone()),
        ];
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
        self.ci_watch(
            run_dir,
            (&run.id, &group),
            Sink::CdRun(&run.id, &sandbox.id),
            &plan,
            &token,
            &log,
            ring,
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
        if run.environment == CdEnv::Prod && !level.allows(Action::Administer) {
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
            &format!("{}/{}", sandbox.slug, run.environment.as_str()),
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
    if let Some(prod) = config.prod.as_mut() {
        prod.promote = false;
    }
    config
}

fn running_refusal(env: CdEnv) -> AppError {
    AppError::Conflict(format!(
        "a deploy to {} is running; wait for it to end (or cancel it)",
        env.label()
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
        if env.name == "dev" {
            format!("[cd.dev] from {}", env.branch)
        } else {
            format!("[cd.prod], deploys {}", env.branch)
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
            .map(|s| format!("{} {}", s.name, s.state.replace('_', " ")))
            .collect();
        out.push_str(&format!("  secrets: {}\n", secrets.join(", ")));
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
            "You may deploy dev (cd_deploy); production is deployed by a person on {}/s/{}/cd.",
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
        let env =
            CdEnv::parse(name).ok_or_else(|| AppError::Invalid("the environment is dev".into()))?;
        let run = self.cd_deploy_for_task(ctx, env).await?;
        let view = self.cd_run_view(&run, false).await;
        let text = format!(
            "Deploying dev: run {} of {} at {}. Start `yulius wait` in the background: it returns when the run ends; then read it with cd_status {{run_id}}.",
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
