//! Production asked for by the Superagent (SPEC-487 slice 8b, REL-001 to REL-005; wall 4):
//! a production-stage deploy or rollback, a promotion and a project's onboarding wait on the
//! Superagent's own panel as its other requests do (ADR-0333), and run the pages' own
//! functions as the person who approved them.
//!
//! Contract:
//! - checked before anyone is asked, as the CD page checks it: an administrator for production,
//!   an environment that exists and is production-stage (a dev-stage one runs at once through
//!   `cd_deploy` and `cd_rollback`), onboarding that waits to start;
//! - approved by that person's click in a browser, with the project's slug typed for a release
//!   (the CD page's own confirmation, ADR-0110), then run as that person, whose rights the
//!   functions check again at that moment; their refusals are the answer.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use yulius_contracts::api::cd::{CdDeploy, CdRollbackRequest};
use yulius_domain::permission::Action;

use crate::auth::AuthContext;
use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::cross_sandbox::CrossAction;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, UserRecord};
use crate::person_tools::record;

/// What a production deploy names.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeployArgs {
    environment: String,
}

/// What a production rollback names.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RollbackArgs {
    environment: String,
    #[serde(default)]
    release: Option<String>,
    #[serde(default)]
    part: Option<String>,
}

/// What a promotion takes: nothing.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PromoteArgs {}

/// What onboarding takes: the person's agent's hand-over, if any (SPEC-256 LOB-005).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OnboardArgs {
    #[serde(default)]
    notes: Option<String>,
}

fn parse<T: serde::de::DeserializeOwned>(args: &Value) -> AppResult<T> {
    serde_json::from_value(args.clone())
        .map_err(|err| AppError::Invalid(format!("invalid arguments: {err}")))
}

/// The person as the pages know them, for a function their page calls.
fn as_person(user: &UserRecord) -> AuthContext {
    AuthContext {
        user: user.clone(),
        csrf_token: None,
        session_hash: None,
        token: None,
    }
}

impl App {
    /// Whether `environment` of the Superagent's sandbox is production-stage, so its deploy
    /// and rollback wait for the person (REL-003). An environment the project does not define
    /// is not: the function the tool calls says so.
    ///
    /// # Errors
    /// The person no longer works in the project; store failures.
    pub async fn superagent_production(
        &self,
        caller: &ProjectCaller,
        environment: &str,
    ) -> AppResult<bool> {
        let user = self.manage_authority(caller).await?;
        let Some(env) = yulius_domain::cd::env_name(environment) else {
            return Ok(false);
        };
        // An overview that cannot be read leaves the call to the tool, whose deploy refuses
        // production without the typed slug anyway (review of slice 8b, m2).
        let Ok(overview) = self
            .cd_overview(&as_person(&user), caller.sandbox.as_str())
            .await
        else {
            return Ok(env == "prod");
        };
        Ok(env == "prod"
            || overview
                .environments
                .iter()
                .any(|e| e.name == env && e.stage == "prod"))
    }

    /// Refuses before anyone is asked what the page would refuse the person now (REL-002).
    pub(crate) async fn checked_release(
        &self,
        user: &UserRecord,
        own: &SandboxRecord,
        action: CrossAction,
        args: &Value,
    ) -> AppResult<()> {
        let environment = match action {
            CrossAction::Deploy => Some(parse::<DeployArgs>(args)?.environment),
            CrossAction::Rollback => Some(parse::<RollbackArgs>(args)?.environment),
            CrossAction::Promote => {
                parse::<PromoteArgs>(args)?;
                None
            }
            CrossAction::Onboard => {
                parse::<OnboardArgs>(args)?;
                self.require(user, &own.id, Action::Work).await?;
                if own.onboarding_state != "pending" {
                    return Err(AppError::Conflict(format!(
                        "onboarding is {}: there is nothing to start",
                        own.onboarding_state
                    )));
                }
                return Ok(());
            }
            _ => return Err(AppError::internal("not a release")),
        };
        if self
            .require(user, &own.id, Action::Administer)
            .await
            .is_err()
        {
            return Err(AppError::Forbidden(
                "only an administrator of this project deploys, rolls back or promotes \
                 production, and your person is not one here: tell them so"
                    .into(),
            ));
        }
        let Some(environment) = environment else {
            return Ok(());
        };
        let env = yulius_domain::cd::env_name(&environment)
            .ok_or_else(|| AppError::Invalid(format!("`{environment}` is no environment name")))?;
        let overview = self.cd_overview(&as_person(user), own.id.as_str()).await?;
        let Some(defined) = overview.environments.iter().find(|e| e.name == env) else {
            let names: Vec<&str> = overview
                .environments
                .iter()
                .map(|e| e.name.as_str())
                .collect();
            return Err(AppError::Invalid(format!(
                "this project has no environment `{env}`; it has: {}",
                names.join(", ")
            )));
        };
        if env != "prod" && defined.stage != "prod" {
            return Err(AppError::Invalid(format!(
                "`{env}` is a dev-stage environment: {} runs it at once, nobody is asked",
                action.own_tool()
            )));
        }
        Ok(())
    }

    /// Runs an approved release as `person`, who answered it in a browser: the CD page's
    /// deploy, rollback or promotion with the project's slug as typed, or the project page's
    /// Start onboarding.
    pub(crate) async fn run_release(
        self: &Arc<Self>,
        person: &AuthContext,
        target: &SandboxRecord,
        action: CrossAction,
        args: &Value,
    ) -> AppResult<Value> {
        let sid = target.id.as_str();
        let slug = Some(target.slug.clone());
        Ok(match action {
            CrossAction::Deploy => {
                let asked: DeployArgs = parse(args)?;
                let req = CdDeploy {
                    confirm: slug,
                    expect: None,
                    confirm_ms: None,
                };
                let run = self
                    .cd_deploy(person, sid, &asked.environment, &req)
                    .await?;
                json!({
                    "outcome": "approved",
                    "run": record(&run)?,
                    "note": "The person approved: the deploy started. Follow it with cd_status (run_id).",
                })
            }
            CrossAction::Rollback => {
                let asked: RollbackArgs = parse(args)?;
                let req = CdRollbackRequest {
                    part: asked.part,
                    release: asked.release,
                    confirm: slug,
                    confirm_ms: None,
                };
                let run = self
                    .cd_rollback(person, sid, &asked.environment, &req)
                    .await?;
                json!({
                    "outcome": "approved",
                    "run": record(&run)?,
                    "note": "The person approved: the rollback started. Follow it with cd_status (run_id).",
                })
            }
            CrossAction::Promote => {
                let promoted = self.promote_sandbox(person, sid, slug.as_deref()).await?;
                json!({
                    "outcome": "approved",
                    "promotion": record(&promoted)?,
                    "note": "The person approved the promotion.",
                })
            }
            CrossAction::Onboard => {
                let asked: OnboardArgs = parse(args)?;
                let view = self
                    .approve_onboarding(person, sid, asked.notes.as_deref())
                    .await?;
                json!({
                    "outcome": "approved",
                    "onboarding": view.onboarding,
                    "note": "The person approved: onboarding started; its task is on the Tasks page.",
                })
            }
            _ => return Err(AppError::internal("not a release")),
        })
    }
}
