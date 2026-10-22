//! Project variables (SPEC-275, ADR-0203): a project's non-secret values (regions, environment
//! names, ids, base URLs, contact addresses), shown in plain to everyone who sees the project,
//! changed by people with write access (and their operate tokens), put into every task run's
//! environment. A name that is an agent secret of the project is refused.

use std::collections::HashSet;

use yulius_contracts::api::variables::{
    ProjectVariableInput, ProjectVariableView, ProjectVariables, ProjectVariablesSetResult,
};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::vault::SecretName;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::variables::{VariableOutcome, VariableWrite};
use crate::ports::vault::AgentScope;

/// Longest value, in bytes.
pub const MAX_VARIABLE_VALUE: usize = 4096;
/// Longest description, in characters.
pub const MAX_VARIABLE_DESCRIPTION: usize = 1000;
/// Variables per project.
pub const MAX_VARIABLES: usize = 200;

/// A checked input: its name, value and description (`None`: keep the stored one).
struct Checked {
    name: String,
    value: String,
    description: Option<String>,
}

fn check(input: &ProjectVariableInput) -> AppResult<Checked> {
    let name = SecretName::parse(&input.name)
        .map_err(|e| AppError::Invalid(e.to_string()))?
        .as_str()
        .to_owned();
    let value = &input.value;
    if value.len() > MAX_VARIABLE_VALUE {
        return Err(AppError::Invalid(format!(
            "{name}: the value is longer than {MAX_VARIABLE_VALUE} bytes"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(AppError::Invalid(format!(
            "{name}: a variable is one line, without control characters"
        )));
    }
    let description = match input.description.as_deref().map(str::trim) {
        Some(text) if text.chars().count() > MAX_VARIABLE_DESCRIPTION => {
            return Err(AppError::Invalid(format!(
                "{name}: the description is longer than {MAX_VARIABLE_DESCRIPTION} characters"
            )));
        }
        Some(text) if text.chars().any(char::is_control) => {
            return Err(AppError::Invalid(format!(
                "{name}: the description is one line, without control characters"
            )));
        }
        other => other.map(str::to_owned),
    };
    Ok(Checked {
        name,
        value: value.clone(),
        description,
    })
}

impl App {
    /// `GET /api/sandboxes/{sid}/variables` (VAR-003): the project's variables in plain, to
    /// anyone who sees it.
    ///
    /// # Errors
    /// `NotFound` for invisible sandboxes; store failures.
    pub async fn project_variables(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
    ) -> AppResult<ProjectVariables> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        let (_, level) = self.require(&ctx.user, &sid, Action::View).await?;
        let variables = self
            .deps
            .store
            .project_variables(&sid)
            .await?
            .into_iter()
            .map(|r| ProjectVariableView {
                name: r.name,
                value: r.value,
                description: r.description,
                updated_by: r.updated_by,
                updated_at: rfc3339(r.updated_at),
            })
            .collect();
        Ok(ProjectVariables {
            variables,
            can_manage: level >= AccessLevel::Write && !ctx.read_only(),
        })
    }

    /// `PUT /api/sandboxes/{sid}/variables` (VAR-001..003): creates or changes each variable of
    /// `inputs` (only creates with `only_new`), all checked before any is written and each outcome
    /// decided in one transaction; audited `variable.create`/`variable.update` per change, by
    /// name, never the value.
    ///
    /// # Errors
    /// Access errors; `Invalid` for a bad name, value or description, a name given twice, more
    /// than [`MAX_VARIABLES`]; `Conflict` for a name that is an agent secret of the project.
    pub async fn set_project_variables(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        inputs: &[ProjectVariableInput],
        only_new: bool,
    ) -> AppResult<ProjectVariablesSetResult> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::Configure).await?;
        if inputs.len() > MAX_VARIABLES {
            return Err(AppError::Invalid(format!(
                "at most {MAX_VARIABLES} variables at once"
            )));
        }
        let checked = inputs.iter().map(check).collect::<AppResult<Vec<_>>>()?;
        let mut seen = HashSet::new();
        if let Some(twice) = checked.iter().find(|c| !seen.insert(c.name.as_str())) {
            return Err(AppError::Invalid(format!("{} is given twice", twice.name)));
        }
        let secrets: HashSet<String> = self
            .deps
            .store
            .vault_agent_in(&sid)
            .await?
            .into_iter()
            .filter(|r| !matches!(r.scope(), AgentScope::Personal(_)))
            .map(|r| r.name)
            .collect();
        let clashes: Vec<&str> = checked
            .iter()
            .map(|c| c.name.as_str())
            .filter(|name| secrets.contains(*name))
            .collect();
        if !clashes.is_empty() {
            return Err(AppError::Conflict(format!(
                "{} {} an agent secret of this project: a name is either a secret or a variable; \
                 nothing was set",
                clashes.join(", "),
                if clashes.len() == 1 { "is" } else { "are" }
            )));
        }

        let actor = Self::variable_actor(ctx);
        let writes = checked
            .into_iter()
            .map(|c| VariableWrite {
                name: c.name,
                value: c.value,
                description: c.description,
            })
            .collect();
        let outcomes = self
            .deps
            .store
            .project_variables_put(&sid, writes, (&actor, self.now()), only_new, MAX_VARIABLES)
            .await?
            .ok_or_else(|| {
                AppError::Invalid(format!(
                    "a project holds at most {MAX_VARIABLES} variables; nothing was set"
                ))
            })?;
        let mut result = ProjectVariablesSetResult::default();
        let mut changed = false;
        for (name, outcome) in outcomes {
            match outcome {
                VariableOutcome::Created => {
                    self.variable_audit(&actor, "variable.create", &sid, &name)
                        .await?;
                    result.created.push(name);
                    changed = true;
                }
                VariableOutcome::Changed(fields) => {
                    let detail = format!("{name} ({})", fields.join(", "));
                    self.variable_audit(&actor, "variable.update", &sid, &detail)
                        .await?;
                    result.changed.push(name);
                    changed = true;
                }
                VariableOutcome::Unchanged => result.unchanged.push(name),
                VariableOutcome::Existing => result.existing.push(name),
            }
        }
        if changed {
            self.announce_variables(&sid);
        }
        Ok(result)
    }

    /// `DELETE /api/sandboxes/{sid}/variables/{name}` (VAR-003): audited `variable.delete`.
    ///
    /// # Errors
    /// Access errors; `NotFound` when there is no such variable.
    pub async fn delete_project_variable(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        name: &str,
    ) -> AppResult<()> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::Configure).await?;
        if !self
            .deps
            .store
            .project_variable_delete(&sid, name.trim())
            .await?
        {
            return Err(AppError::NotFound);
        }
        let actor = Self::variable_actor(ctx);
        self.variable_audit(&actor, "variable.delete", &sid, name.trim())
            .await?;
        self.announce_variables(&sid);
        Ok(())
    }

    /// The environment every task run of `sandbox` starts from (VAR-004): its variables, by name.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn variables_env(
        &self,
        sandbox: &SandboxId,
    ) -> AppResult<Vec<(String, String)>> {
        Ok(self
            .deps
            .store
            .project_variables(sandbox)
            .await?
            .into_iter()
            .map(|r| (r.name, r.value))
            .collect())
    }

    /// The person's audit label (their login), as for agent secrets.
    fn variable_actor(ctx: &AuthContext) -> String {
        ctx.user
            .github_login
            .clone()
            .unwrap_or_else(|| "superadmin".into())
    }

    async fn variable_audit(
        &self,
        actor: &str,
        action: &str,
        sandbox: &SandboxId,
        detail: &str,
    ) -> AppResult<()> {
        self.deps
            .store
            .audit(
                self.now(),
                actor,
                action,
                Some(sandbox.as_str()),
                Some(detail),
            )
            .await
    }

    /// Open Secrets pages of the project read its variables again.
    fn announce_variables(&self, sandbox: &SandboxId) {
        self.events.publish(
            Some(sandbox.clone()),
            false,
            yulius_contracts::events::UiEvent::SecretsUpdated {
                sandbox_id: Some(sandbox.to_string()),
            },
        );
    }
}
