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

/// What `secret_request_value` takes (SPEC-487 SEC-001).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SecretRequestArgs {
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

/// The longest description of a secret asked for.
const MAX_SECRET_DESCRIPTION: usize = 500;

/// The scope a secret is asked in: `sandbox` (the default) or `personal`.
fn asked_scope(asked: &SecretRequestArgs) -> &str {
    asked.scope.as_deref().unwrap_or("sandbox")
}

/// The value the vault stores for what the person typed (SEC-002): as the Secrets page stores a
/// pasted value, a key's line breaks written as literal `\n` (ADR-0156); one it refuses is
/// refused here, naming the rule, never quoting the value.
pub(crate) fn secret_value_to_store(typed: Option<&str>) -> AppResult<zeroize::Zeroizing<String>> {
    let typed = typed.filter(|v| !v.trim().is_empty()).ok_or_else(|| {
        AppError::Invalid("type the secret's value to store it, or decline".into())
    })?;
    yulius_domain::vault::dotenv::agent_value_from_file(typed)
        .map(zeroize::Zeroizing::new)
        .map_err(|err| AppError::Invalid(format!("the value cannot be stored: {err}")))
}

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

/// Whether `environment` is the overview's merge-first staging target (dev stage, with a queue).
fn is_staging(cd: &yulius_contracts::api::cd::CdOverview, environment: &str) -> bool {
    yulius_domain::cd::env_name(environment).is_some_and(|env| {
        cd.environments
            .iter()
            .any(|e| e.name == env && e.stage != "prod" && e.staging.is_some())
    })
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
    /// Refuses before anyone is asked a secret's value that could not be stored as asked
    /// (SEC-001; review of slice 10a, M3): a name the vault takes from an agent (capitals,
    /// digits and underscores, not reserved, not the Vidoc key), a scope of `sandbox` (default:
    /// the person needs Configure in the project) or `personal`, a description of at most 500
    /// characters.
    pub(crate) async fn checked_secret_request(
        &self,
        user: &UserRecord,
        own: &SandboxRecord,
        args: &Value,
    ) -> AppResult<()> {
        let asked: SecretRequestArgs = parse(args)?;
        let name = asked.name.trim();
        let shaped = name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !shaped {
            return Err(AppError::Invalid(
                "the name is capitals, digits and underscores, starting with a capital (API_TOKEN)"
                    .into(),
            ));
        }
        let proposed = crate::vault::create::agent_made_name(name)?;
        // A request may rotate a previously accepted name in this person's requested scope.
        // If that row disappears before the answer, creation checks the name again.
        if let Err(error) = proposed.check_assignment()
            && self
                .secret_replaced(&as_person(user), own.id.as_str(), args)
                .await
                .is_none()
        {
            return Err(AppError::Invalid(error.to_string()));
        }
        if asked
            .description
            .as_ref()
            .is_some_and(|d| d.chars().count() > MAX_SECRET_DESCRIPTION)
        {
            return Err(AppError::Invalid(format!(
                "the description is at most {MAX_SECRET_DESCRIPTION} characters"
            )));
        }
        match asked_scope(&asked) {
            "personal" => Ok(()),
            "sandbox" => self
                .require(user, &own.id, Action::Configure)
                .await
                .map(|_| ())
                .map_err(|_| {
                    AppError::Forbidden(
                        "your person may not add this project's secrets (Configure): ask for \
                         a personal one (scope personal), or tell them so"
                            .into(),
                    )
                }),
            _ => Err(AppError::Invalid(
                "scope is sandbox (the project's, the default) or personal (the person's own)"
                    .into(),
            )),
        }
    }

    /// Whether the person already has a secret of that name in the scope asked: the one a
    /// value replaces (review of slice 10a, M1, M2). Never a global one, nor another scope's.
    pub(crate) async fn secret_replaced(
        &self,
        person: &AuthContext,
        sandbox: &str,
        args: &Value,
    ) -> Option<String> {
        let asked: SecretRequestArgs = parse(args).ok()?;
        let scope = asked_scope(&asked).to_owned();
        self.agent_secrets(person, sandbox)
            .await
            .ok()?
            .into_iter()
            .find(|s| s.name == asked.name.trim() && s.scope == scope)
            .map(|s| s.id)
    }

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

    /// Whether `environment` of the Superagent's sandbox is its merge-first staging target, so
    /// its deploy waits for the person on the panel too and then asks the staging queue
    /// (SPEC-650 PST-004). An overview that cannot be read reads as no.
    ///
    /// # Errors
    /// The person no longer works in the project; store failures.
    pub async fn superagent_staging(
        &self,
        caller: &ProjectCaller,
        environment: &str,
    ) -> AppResult<bool> {
        let user = self.manage_authority(caller).await?;
        let Some(env) = yulius_domain::cd::env_name(environment) else {
            return Ok(false);
        };
        Ok(self
            .cd_overview(&as_person(&user), caller.sandbox.as_str())
            .await
            .is_ok_and(|cd| is_staging(&cd, &env)))
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
        // The merge-first staging target asks its queue with write access (SPEC-650 PST-004).
        if action == CrossAction::Deploy
            && let Some(environment) = &environment
            && self
                .cd_overview(&as_person(user), own.id.as_str())
                .await
                .is_ok_and(|cd| is_staging(&cd, environment))
        {
            return self
                .require(user, &own.id, Action::Work)
                .await
                .map(|_| ())
                .map_err(|_| {
                    AppError::Forbidden(
                        "asking for staging needs write access to this project, and your \
                         person has none here: tell them so"
                            .into(),
                    )
                });
        }
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
                // The merge-first staging target: the person's request to its queue (PST-004).
                if self
                    .cd_overview(person, sid)
                    .await
                    .is_ok_and(|cd| is_staging(&cd, &asked.environment))
                {
                    let request = self.cd_staging_ask(person, sid, &asked.environment).await?;
                    return Ok(json!({
                        "outcome": "approved",
                        "staging": record(&request)?,
                        "note": format!(
                            "The person approved: staging request {} for revision {} is {}{}. It deploys once its full gate passed, or the next green revision; follow it with cd_status.",
                            request.id,
                            request.revision.get(..12).unwrap_or(&request.revision),
                            request.state,
                            request.waiting.as_deref().map_or_else(String::new, |w| format!(" ({w})")),
                        ),
                    }));
                }
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

    /// Stores a secret's value its person typed on the panel (SEC-003), as the Secrets page
    /// does with their session: a new agent secret, or the new value of theirs of that name.
    /// Answers its name and whether it was created or updated, never the value.
    pub(crate) async fn store_secret_value(
        &self,
        person: &AuthContext,
        target: &SandboxRecord,
        args: &Value,
        value: Option<zeroize::Zeroizing<String>>,
    ) -> AppResult<Value> {
        let asked: SecretRequestArgs = parse(args)?;
        let value = value.ok_or_else(|| AppError::internal("no value was taken"))?;
        let name = asked.name.trim().to_owned();
        let stored = if let Some(id) = self.secret_replaced(person, target.id.as_str(), args).await
        {
            let patch = yulius_contracts::api::secrets::AgentSecretPatch {
                value: Some(value.to_string()),
                ..yulius_contracts::api::secrets::AgentSecretPatch::default()
            };
            self.update_agent_secret(person, &id, &patch).await?;
            "updated"
        } else {
            let input = yulius_contracts::api::secrets::AgentSecretInput {
                scope: Some(asked_scope(&asked).to_owned()),
                name: name.clone(),
                description: asked
                    .description
                    .as_deref()
                    .map(str::trim)
                    .map(str::to_owned),
                hosts: Vec::new(),
                rules: None,
                injection: None,
                header_name: None,
                username: None,
                value: value.to_string(),
                auto_allow: None,
                env_files: None,
            };
            self.create_agent_secret(person, target.id.as_str(), &input)
                .await?;
            "created"
        };
        Ok(json!({
            "outcome": "approved",
            "secret": name,
            "stored": stored,
            "note": format!("The person stored {name} in the vault; you never see its value: use it as ${name} or its placeholder (secrets_list says how)."),
        }))
    }
}
