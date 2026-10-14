//! CD (deployment): runs of a project's dev and production environments (SPEC-134, ADR-0109,
//! ADR-0110).
//!
//! A CD run deploys one environment from its protected branch: Yulius fetches, resolves the
//! branch head, reads `[cd.<env>]` from `yulius.toml` **at that commit** (never from a task
//! branch, a worktree or the database), checks the commit out into the environment's own
//! checkout and runs the `deploy` command and then the `verify` command through the process host,
//! inside the sandbox's containment, like a CI job. Dev reads the integration branch; production
//! the commit it deploys (the integration head it promotes first when `[cd.prod] promote = true`
//! there, else the production head).
//!
//! Trust contract:
//! - who may deploy is [`yulius_domain::cd::authorize`]: dev is a person's (write access) or an
//!   agent's; production only an administrator's who typed the project's slug, never an agent's;
//! - a run's environment has no task token, provider credential or GitHub token; it has the real
//!   values of the secrets its table names, only those a person allowed outside the proxy
//!   (ADR-0105), and a `HOME` made for the run and removed after it;
//! - stored output is redacted of every agent secret of the sandbox;
//! - one running run per environment; a run left running by a restart is recorded as `error` and
//!   never run again by itself (a deploy is not assumed repeatable);
//! - every run is audited before it starts (no audit entry, no run), and its end.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::watch;
use yulius_contracts::api::cd::{CdDeploy, CdEnvironment, CdOverview, CdRun, CdSecret};
use yulius_domain::cd::{self as dom, CdConfig, CdEnv, CdRefusal, CdState, Requester};
use yulius_domain::ci::RunState;
use yulius_domain::delivery::DeliveryProfile;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::spec_format::{MAX_CONFIG_BYTES, parse_yulius_toml_cd};
use yulius_domain::time::Timestamp;

use super::OUTPUT_BYTES;
use super::exec::{Ended, Sink};
use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::coordination::AgentContext;
use crate::delivery::delivery_profile;
use crate::delivery::runner::OutputRing;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, UserRecord};
use crate::ports::session::LaunchPlan;

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

/// An environment's configuration and where it comes from.
struct CdSource {
    /// The branch deployed.
    branch: String,
    /// Its head.
    head: Option<String>,
    /// `[cd.*]` as the source commit defines it, or why it cannot be read.
    config: Result<CdConfig, String>,
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

impl App {
    fn cd_dir(&self, sandbox: &SandboxId) -> PathBuf {
        self.sandbox_dir(sandbox).join("cd")
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

    /// Where `env` comes from in the clone as last fetched (CDC-003): dev from the integration
    /// head; production from the integration head when that head's `[cd.prod]` promotes, else
    /// from the production head. The branch is the one deployed (production's for `prod`).
    async fn cd_source(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        env: CdEnv,
    ) -> CdSource {
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
            return CdSource {
                branch: profile.integration_branch.clone(),
                head: integration,
                config: integration_config,
            };
        }
        let production = head(profile.production_branch.clone()).await;
        let promotes = integration_config
            .as_ref()
            .is_ok_and(|config| config.prod.as_ref().is_some_and(|prod| prod.promote));
        let config = if promotes {
            integration_config
        } else {
            match &production {
                Some(sha) => self.cd_config_at(&repo, sha).await,
                None => Ok(CdConfig::default()),
            }
        };
        CdSource {
            branch: profile.production_branch.clone(),
            head: production,
            config,
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
            .cd_runs(&sandbox.id, CD_RUN_LIST_LIMIT.max(CD_KEEP_RUNS))
            .await?;
        let mut environments = Vec::with_capacity(2);
        for env in CdEnv::ALL {
            let source = self.cd_source(sandbox, &profile, env).await;
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
                    .cd_secret_states(&sandbox.id, &target.secrets)
                    .await?
                    .into_iter()
                    .map(|(name, state)| CdSecret {
                        name,
                        state: state.as_str().to_owned(),
                    })
                    .collect(),
                None => Vec::new(),
            };
            let mine = runs.iter().filter(|run| run.environment == env);
            let deployed = mine
                .clone()
                .find(|run| run.state == RunState::Passed)
                .map(|run| run.sha.clone());
            let last_run = match mine.clone().next() {
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
                secrets,
                deployed,
                last_run,
            });
        }
        let mut views = Vec::new();
        for run in runs.iter().take(CD_RUN_LIST_LIMIT as usize) {
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
            .cd_start(&sandbox, env, requester, &CdActor::Person(&ctx.user))
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
        self.cd_start(&sandbox, env, Requester::Agent, &CdActor::Agent(ctx))
            .await
    }

    /// Checks everything, promotes when production says so, records the run and starts it.
    async fn cd_start(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        env: CdEnv,
        requester: Requester<'_>,
        actor: &CdActor<'_>,
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
        let profile = delivery_profile(sandbox);
        self.dlv_refresh_clone(sandbox).await?;
        let source = self.cd_source(sandbox, &profile, env).await;
        let config = source.config.map_err(|problem| {
            AppError::Conflict(format!(
                "CD can't be read on '{}': {problem}",
                if env == CdEnv::Dev {
                    &profile.integration_branch
                } else {
                    &source.branch
                }
            ))
        })?;
        dom::authorize(&config, env, requester, &sandbox.slug)
            .map_err(|refused| refusal(&refused))?;
        let Some(target) = config.target(env).cloned() else {
            return Err(AppError::Internal(
                "an authorized environment has a table".into(),
            ));
        };
        if self
            .deps
            .store
            .cd_runs(&sandbox.id, CD_KEEP_RUNS)
            .await?
            .iter()
            .any(|run| run.environment == env && run.state == RunState::Running)
        {
            return Err(AppError::Conflict(format!(
                "a deploy to {} is running; wait for it to end (or cancel it)",
                env.label()
            )));
        }
        let asker = match actor {
            CdActor::Agent(ctx) => Some(*ctx),
            CdActor::Person(..) => None,
        };
        let secrets = self
            .cd_secret_values(
                sandbox,
                &target.secrets,
                asker,
                &format!("CD {} deploy", env.label()),
            )
            .await?;
        let mut note = None;
        if env == CdEnv::Prod && target.promote {
            let CdActor::Person(user) = actor else {
                return Err(refusal(&CdRefusal::ProductionIsAPersons));
            };
            note = Some(self.cd_promote(sandbox, &profile, user).await?);
            self.dlv_refresh_clone(sandbox).await?;
        }
        let repo = self.sandbox_repo(&sandbox.id);
        let sha = self
            .deps
            .git
            .resolve_commit(&repo, &format!("origin/{}", source.branch))
            .await?
            .ok_or_else(|| {
                AppError::Conflict(format!("branch '{}' has no head to deploy", source.branch))
            })?;
        let run = CdRunRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: sandbox.id.clone(),
            environment: env,
            branch: source.branch.clone(),
            sha: sha.clone(),
            state: RunState::Running,
            reason: None,
            requested_by: actor.label(),
            task_id: asker.map(|ctx| ctx.task.id.clone()),
            deploy: target.deploy.clone(),
            verify: target.verify.clone(),
            note,
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
                AppError::Conflict(_) => AppError::Conflict(format!(
                    "a deploy to {} is running; wait for it to end (or cancel it)",
                    env.label()
                )),
                other => other,
            })?;
        let detail = format!(
            "run {} of {} at {sha}; deploy: {}; verify: {}; secrets: [{}]{}",
            run.id,
            source.branch,
            target.deploy.as_deref().unwrap_or("-"),
            target.verify.as_deref().unwrap_or("-"),
            target.secrets.join(" "),
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
            app.cd_execute(sandbox, started, secrets, rx).await;
        });
        Ok(run)
    }

    /// Promotes the integration branch for a person's production deploy: nothing when
    /// production already contains it; otherwise the profile's promotion, and with a merge-commit
    /// promotion the pull request is merged at once when its checks passed. Returns the note the
    /// run keeps.
    async fn cd_promote(
        &self,
        sandbox: &SandboxRecord,
        profile: &DeliveryProfile,
        user: &UserRecord,
    ) -> AppResult<String> {
        let (integration, production) = (&profile.integration_branch, &profile.production_branch);
        let ahead = self
            .dlv_branch_relation(sandbox, profile, true)
            .await
            .map(|(ahead, _)| ahead);
        if ahead == Some(0) {
            return Ok(format!(
                "'{production}' already contains '{integration}': nothing to promote"
            ));
        }
        let token = self
            .person_github_token(user, &format!("Deploying {production}"))
            .await?;
        let mut result = self.cd_promote_once(sandbox, profile, user, &token).await?;
        if result.result == "pr_opened" {
            let url = result.sha.clone().unwrap_or_default();
            result = self
                .cd_promote_once(sandbox, profile, user, &token)
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
        token: &crate::model::Secret,
    ) -> AppResult<yulius_contracts::api::git::PromoteResult> {
        self.dlv_promote(
            sandbox,
            profile,
            yulius_domain::delivery::Initiator::User,
            user,
            token,
            true,
        )
        .await
    }

    /// Background part of a run: checkout, the commands in the sandbox, the record of the end.
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
        let home = self.cd_dir(&sandbox.id).join("homes").join(&run.id);
        let ended = self
            .cd_process(&sandbox, &run, secrets, &home, &mut ring, &mut cancel)
            .await;
        let _ = std::fs::remove_dir_all(&home);
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
        let output = self.redact_for_sandbox(&sandbox.id, &ring.text()).await;
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

    #[allow(clippy::too_many_arguments)] // One run's whole context.
    async fn cd_process(
        &self,
        sandbox: &SandboxRecord,
        run: &CdRunRecord,
        secrets: Vec<(String, String)>,
        home: &Path,
        ring: &mut OutputRing,
        cancel: &mut watch::Receiver<Option<String>>,
    ) -> Result<Ended, String> {
        let cd_dir = self.cd_dir(&sandbox.id);
        let checkout = cd_dir.join("checkouts").join(run.environment.as_str());
        let (cache, logs) = (cd_dir.join("cache"), cd_dir.join("logs"));
        for dir in [&cache, &logs, home] {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("preparing the CD directory failed: {e}"))?;
        }
        let repo = self.sandbox_repo(&sandbox.id);
        self.deps
            .git
            .ci_checkout(&repo, &checkout, &run.sha, &cd_dir)
            .await
            .map_err(|err| format!("checking out {} failed: {}", run.sha, err.public_message()))?;
        let log = logs.join(format!("{}.log", run.id));
        let _ = std::fs::remove_file(&log);
        std::fs::File::create(&log).map_err(|e| format!("creating the run log failed: {e}"))?;
        // Held until the run ends: the sandbox's holder is not stopped as idle meanwhile.
        let (join, _lease) = match self.isolation_join(sandbox).await {
            Ok(joined) => joined.unzip(),
            Err(err) => return Err(err.public_message()),
        };
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
            cwd: checkout,
            env,
            secret_env: secrets,
            files: Vec::new(),
            containment: self.ci_containment(sandbox, home, join),
        };
        let run_dir = self
            .config
            .data_dir
            .join("runs")
            .join(format!("cd-{}", run.id));
        let _ = std::fs::remove_dir_all(&run_dir);
        std::fs::create_dir_all(&run_dir)
            .map_err(|e| format!("creating the run directory failed: {e}"))?;
        let token = self.deps.crypto.random_bytes();
        let group = format!("cd-{}", run.id);
        let outcome = self
            .ci_watch(
                &run_dir,
                (&run.id, &group),
                Sink::CdRun(&run.id, &sandbox.id),
                &plan,
                &token,
                &log,
                ring,
                cancel,
            )
            .await;
        let _ = std::fs::remove_dir_all(&run_dir);
        let _ = std::fs::remove_file(&log);
        outcome
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

    /// `POST /api/sandboxes/{sid}/cd/runs/{rid}/cancel` (write; a production run: administrator).
    /// Kills the run's process group; a run without an executor (lost) is recorded cancelled.
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
            let output = format!("{}\n[yulius] {reason}\n", run.output);
            self.deps
                .store
                .finish_cd_run(
                    &run.id,
                    &CdRunEnd {
                        state: RunState::Cancelled,
                        reason: Some(reason.clone()),
                        exit_code: None,
                        output,
                        ended_at: self.now(),
                    },
                )
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

    /// Startup: runs recorded as running have no executor after a restart. They are recorded as
    /// `error` and never started again by themselves (CDR-004). Returns how many. Call once
    /// before serving.
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
            let reason = "Yulius restarted while it ran: its outcome is unknown, and it is not \
                          run again by itself"
                .to_owned();
            let full = self.deps.store.cd_run(&run.id, true).await?.unwrap_or(run);
            let ended = self
                .deps
                .store
                .finish_cd_run(
                    &full.id,
                    &CdRunEnd {
                        state: RunState::Error,
                        reason: Some(reason.clone()),
                        exit_code: None,
                        output: format!("{}\n[yulius] {reason}\n", full.output),
                        ended_at: self.now(),
                    },
                )
                .await?;
            if ended {
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

    /// The recent CD runs `task`'s agent started, newest first (`yulius wait`, CDT-003).
    pub(crate) async fn cd_runs_of_task(
        &self,
        task: &crate::model::TaskRecord,
    ) -> AppResult<Vec<CdRunRecord>> {
        Ok(self
            .deps
            .store
            .cd_runs(&task.sandbox_id, CD_RUN_LIST_LIMIT)
            .await?
            .into_iter()
            .filter(|run| run.task_id.as_ref() == Some(&task.id))
            .collect())
    }
}
