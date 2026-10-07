//! Workflows: reusable task recipes with initial steps, instructions and gates (SPEC-001
//! AUT-004, AC-AUT-3).
//!
//! Instance workflows (`sandbox_id = None`) are visible in every sandbox and only instance
//! administrators may create, edit or delete them; sandbox workflows need write access to their
//! sandbox. Two built-in instance workflows ("Feature" and "Quick task") are seeded whenever the
//! instance has no instance workflow at all, so a fresh instance always offers them.

use serde_json::{Value, json};
use yulius_contracts::api::workflows::{Workflow, WorkflowGates};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::time::Timestamp;

use crate::auth::AuthContext;
use crate::context::App;
use crate::coordination::{AgentContext, MAX_STEPS};
use crate::error::{AppError, AppResult};
use crate::model::TaskRecord;

/// Longest workflow name.
pub const MAX_NAME_CHARS: usize = 80;
/// Longest description.
pub const MAX_DESCRIPTION_CHARS: usize = 2_000;
/// Longest step title.
pub const MAX_STEP_CHARS: usize = 200;
/// Largest instructions text.
pub const MAX_INSTRUCTIONS_BYTES: usize = 16 * 1024;
/// Most workflows per scope (the instance, or one sandbox).
pub const MAX_WORKFLOWS_PER_SCOPE: u32 = 200;
/// Id of the built-in "Feature" workflow.
pub const BUILTIN_FEATURE_ID: &str = "wf-feature";
/// Id of the built-in "Quick task" workflow.
pub const BUILTIN_QUICK_TASK_ID: &str = "wf-quick-task";

/// A stored workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRecord {
    /// Opaque id.
    pub id: String,
    /// Owning sandbox (`None` = instance scope).
    pub sandbox_id: Option<SandboxId>,
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Initial step titles.
    pub steps: Vec<String>,
    /// Extra instructions for the agent's briefing.
    pub instructions: String,
    /// A valid spec is required before delivery.
    pub require_spec: bool,
    /// The project test command must pass before delivery.
    pub require_tests: bool,
    /// Creation time.
    pub created_at: Timestamp,
}

impl WorkflowRecord {
    /// `instance` or `sandbox`.
    #[must_use]
    pub fn scope(&self) -> &'static str {
        if self.sandbox_id.is_some() {
            "sandbox"
        } else {
            "instance"
        }
    }

    /// Whether the workflow can be used in `sandbox`.
    #[must_use]
    pub fn visible_in(&self, sandbox: &SandboxId) -> bool {
        self.sandbox_id.as_ref().is_none_or(|s| s == sandbox)
    }
}

/// The built-in instance workflows seeded on first use.
#[must_use]
pub fn builtin_workflows(now: Timestamp) -> Vec<WorkflowRecord> {
    vec![
        WorkflowRecord {
            id: BUILTIN_FEATURE_ID.to_owned(),
            sandbox_id: None,
            name: "Feature".to_owned(),
            description: "Spec first, then implementation, tests and a pull request.".to_owned(),
            steps: vec![
                "Write spec with plan".to_owned(),
                "Implement".to_owned(),
                "Test".to_owned(),
                "Open PR".to_owned(),
            ],
            instructions: "Start by writing the task spec with a Plan checklist in the \
                           project's spec directory and keep the steps in sync with it. \
                           Implement only what the spec describes, add tests that prove the \
                           behavior, run the project's test command and open a pull request \
                           into the integration branch."
                .to_owned(),
            require_spec: true,
            require_tests: true,
            created_at: now,
        },
        WorkflowRecord {
            id: BUILTIN_QUICK_TASK_ID.to_owned(),
            sandbox_id: None,
            name: "Quick task".to_owned(),
            description: "A small change without required steps or gates.".to_owned(),
            steps: Vec::new(),
            instructions: String::new(),
            require_spec: false,
            require_tests: false,
            created_at: now,
        },
    ]
}

/// API view of a workflow.
#[must_use]
pub fn workflow_view(record: &WorkflowRecord) -> Workflow {
    Workflow {
        id: record.id.clone(),
        name: record.name.clone(),
        description: record.description.clone(),
        steps: record.steps.clone(),
        instructions: record.instructions.clone(),
        gates: WorkflowGates {
            require_spec: record.require_spec,
            require_tests: record.require_tests,
        },
        scope: record.scope().to_owned(),
    }
}

/// Validated editable fields of a workflow.
struct WorkflowFields {
    name: String,
    description: String,
    steps: Vec<String>,
    instructions: String,
    gates: WorkflowGates,
}

fn validate(input: &Workflow) -> AppResult<WorkflowFields> {
    let name = input.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(AppError::Invalid(format!(
            "the workflow name must be 1 to {MAX_NAME_CHARS} characters"
        )));
    }
    let description = input.description.trim();
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(AppError::Invalid(format!(
            "the description must be at most {MAX_DESCRIPTION_CHARS} characters"
        )));
    }
    if input.steps.len() > MAX_STEPS {
        return Err(AppError::Invalid(format!(
            "a workflow has at most {MAX_STEPS} steps"
        )));
    }
    let mut steps = Vec::with_capacity(input.steps.len());
    for step in &input.steps {
        let step = step.trim();
        if step.is_empty() || step.chars().count() > MAX_STEP_CHARS {
            return Err(AppError::Invalid(format!(
                "each step title must be 1 to {MAX_STEP_CHARS} characters"
            )));
        }
        steps.push(step.to_owned());
    }
    if input.instructions.len() > MAX_INSTRUCTIONS_BYTES {
        return Err(AppError::Invalid(
            "the instructions must be at most 16 KiB".into(),
        ));
    }
    Ok(WorkflowFields {
        name: name.to_owned(),
        description: description.to_owned(),
        steps,
        instructions: input.instructions.trim().to_owned(),
        gates: input.gates,
    })
}

impl App {
    /// Seeds the built-in instance workflows when the instance has none.
    ///
    /// # Errors
    /// Store failures.
    pub async fn ensure_builtin_workflows(&self) -> AppResult<()> {
        let seeded = self
            .deps
            .store
            .seed_instance_workflows(&builtin_workflows(self.now()))
            .await?;
        if seeded {
            self.deps
                .store
                .audit(
                    self.now(),
                    "system",
                    "workflow.seeded",
                    None,
                    Some("Feature, Quick task"),
                )
                .await?;
        }
        Ok(())
    }

    /// Workflows usable in a sandbox (instance first), seeding the built-ins on first use.
    ///
    /// # Errors
    /// Store failures.
    pub async fn workflows_for(&self, sandbox: &SandboxId) -> AppResult<Vec<WorkflowRecord>> {
        self.ensure_builtin_workflows().await?;
        self.deps.store.workflows(sandbox).await
    }

    /// A workflow usable in `sandbox`.
    ///
    /// # Errors
    /// `Invalid` when the workflow does not exist or belongs to another sandbox.
    pub async fn visible_workflow(
        &self,
        sandbox: &SandboxId,
        id: &str,
    ) -> AppResult<WorkflowRecord> {
        self.ensure_builtin_workflows().await?;
        self.deps
            .store
            .workflow(id)
            .await?
            .filter(|w| w.visible_in(sandbox))
            .ok_or_else(|| AppError::Invalid(format!("unknown workflow `{id}`")))
    }

    /// The workflow a task was created from, if it still exists.
    ///
    /// # Errors
    /// Store failures.
    pub async fn task_workflow(&self, task: &TaskRecord) -> AppResult<Option<WorkflowRecord>> {
        let Some(id) = task.workflow_id.as_deref().filter(|id| !id.is_empty()) else {
            return Ok(None);
        };
        Ok(self
            .deps
            .store
            .workflow(id)
            .await?
            .filter(|w| w.visible_in(&task.sandbox_id)))
    }

    /// The task's workflow instructions for its briefing (`None` without a workflow, with
    /// empty instructions, or when the workflow cannot be read — the failure is logged).
    pub async fn workflow_instructions(&self, task: &TaskRecord) -> Option<String> {
        match self.task_workflow(task).await {
            Ok(workflow) => workflow
                .map(|w| w.instructions)
                .filter(|text| !text.trim().is_empty()),
            Err(err) => {
                tracing::warn!(error = %err, task = %task.id, "workflow instructions unavailable");
                None
            }
        }
    }

    /// `GET /api/sandboxes/{sid}/workflows`.
    ///
    /// # Errors
    /// `NotFound` without access.
    pub async fn list_workflows(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
    ) -> AppResult<Vec<Workflow>> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::View).await?;
        Ok(self
            .workflows_for(&sid)
            .await?
            .iter()
            .map(workflow_view)
            .collect())
    }

    /// `POST /api/sandboxes/{sid}/workflows`; `scope: "instance"` creates an instance workflow
    /// (instance administrators only).
    ///
    /// # Errors
    /// Access, validation and capacity errors.
    pub async fn create_workflow(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        input: &Workflow,
    ) -> AppResult<Workflow> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        let instance = match input.scope.as_str() {
            "instance" => true,
            "" | "sandbox" => false,
            _ => {
                return Err(AppError::Invalid(
                    "scope must be instance or sandbox".into(),
                ));
            }
        };
        if instance {
            self.require(&ctx.user, &sid, Action::View).await?;
            self.require_instance_admin(ctx)?;
        } else {
            self.require(&ctx.user, &sid, Action::Configure).await?;
        }
        let fields = validate(input)?;
        self.ensure_builtin_workflows().await?;
        let record = WorkflowRecord {
            id: self.deps.crypto.new_id(),
            sandbox_id: (!instance).then_some(sid),
            name: fields.name,
            description: fields.description,
            steps: fields.steps,
            instructions: fields.instructions,
            require_spec: fields.gates.require_spec,
            require_tests: fields.gates.require_tests,
            created_at: self.now(),
        };
        self.deps
            .store
            .insert_workflow(&record, MAX_WORKFLOWS_PER_SCOPE)
            .await?;
        self.audit_workflow(ctx, "workflow.created", &record)
            .await?;
        Ok(workflow_view(&record))
    }

    async fn workflow_for_change(&self, ctx: &AuthContext, id: &str) -> AppResult<WorkflowRecord> {
        let record = self
            .deps
            .store
            .workflow(id)
            .await?
            .ok_or(AppError::NotFound)?;
        match &record.sandbox_id {
            Some(sid) => {
                self.require(&ctx.user, sid, Action::Configure).await?;
            }
            None => self.require_instance_admin(ctx)?,
        }
        Ok(record)
    }

    /// `PUT /api/workflows/{id}` (scope cannot change).
    ///
    /// # Errors
    /// Access and validation errors.
    pub async fn update_workflow(
        &self,
        ctx: &AuthContext,
        id: &str,
        input: &Workflow,
    ) -> AppResult<Workflow> {
        let current = self.workflow_for_change(ctx, id).await?;
        let fields = validate(input)?;
        let record = WorkflowRecord {
            name: fields.name,
            description: fields.description,
            steps: fields.steps,
            instructions: fields.instructions,
            require_spec: fields.gates.require_spec,
            require_tests: fields.gates.require_tests,
            ..current
        };
        if !self.deps.store.update_workflow(&record).await? {
            return Err(AppError::NotFound);
        }
        self.audit_workflow(ctx, "workflow.updated", &record)
            .await?;
        Ok(workflow_view(&record))
    }

    /// `DELETE /api/workflows/{id}`. Automations using it continue without a workflow; tasks
    /// keep their steps.
    ///
    /// # Errors
    /// Access errors.
    pub async fn delete_workflow(&self, ctx: &AuthContext, id: &str) -> AppResult<()> {
        let record = self.workflow_for_change(ctx, id).await?;
        if !self.deps.store.delete_workflow(&record.id).await? {
            return Err(AppError::NotFound);
        }
        self.audit_workflow(ctx, "workflow.deleted", &record).await
    }

    async fn audit_workflow(
        &self,
        ctx: &AuthContext,
        action: &str,
        record: &WorkflowRecord,
    ) -> AppResult<()> {
        self.deps
            .store
            .audit(
                self.now(),
                &self.login_of(&ctx.user.id).await,
                action,
                Some(&record.id),
                Some(&format!("{} ({})", record.name, record.scope())),
            )
            .await
    }

    /// `workflows_list` agent tool: workflows available in the agent's sandbox (AUT-004).
    ///
    /// # Errors
    /// Store failures.
    pub async fn tool_workflows_list(&self, ctx: &AgentContext) -> AppResult<Value> {
        let workflows = self.workflows_for(&ctx.task.sandbox_id).await?;
        Ok(json!({
            "workflows": workflows.iter().map(|w| json!({
                "id": w.id,
                "name": w.name,
                "description": w.description,
                "scope": w.scope(),
                "steps": w.steps,
                "gates": {"require_spec": w.require_spec, "require_tests": w.require_tests},
            })).collect::<Vec<_>>(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, steps: Vec<String>) -> Workflow {
        Workflow {
            id: String::new(),
            name: name.to_owned(),
            description: String::new(),
            steps,
            instructions: String::new(),
            gates: WorkflowGates::default(),
            scope: String::new(),
        }
    }

    #[test]
    fn builtins_match_the_brief() {
        let builtins = builtin_workflows(Timestamp(5));
        assert_eq!(builtins.len(), 2);
        let feature = &builtins[0];
        assert_eq!(feature.name, "Feature");
        assert_eq!(
            feature.steps,
            ["Write spec with plan", "Implement", "Test", "Open PR"]
        );
        assert!(feature.require_spec && feature.require_tests);
        let quick = &builtins[1];
        assert_eq!(quick.name, "Quick task");
        assert!(quick.steps.is_empty() && !quick.require_spec && !quick.require_tests);
        assert!(builtins.iter().all(|w| w.scope() == "instance"));
    }

    #[test]
    fn validation_bounds() {
        assert!(validate(&input("  ", Vec::new())).is_err());
        assert!(validate(&input(&"x".repeat(MAX_NAME_CHARS + 1), Vec::new())).is_err());
        assert!(validate(&input("ok", vec![" ".into()])).is_err());
        assert!(validate(&input("ok", vec!["s".into(); MAX_STEPS + 1])).is_err());
        let ok = validate(&input("  Release  ", vec!["  Bump  ".into()]));
        assert!(matches!(ok, Ok(f) if f.name == "Release" && f.steps == ["Bump"]));
    }

    #[test]
    fn visibility() -> Result<(), yulius_domain::ids::IdError> {
        let s1 = SandboxId::parse("s1")?;
        let s2 = SandboxId::parse("s2")?;
        let mut record = builtin_workflows(Timestamp(1)).remove(1);
        assert!(record.visible_in(&s1) && record.visible_in(&s2));
        record.sandbox_id = Some(s1.clone());
        assert!(record.visible_in(&s1) && !record.visible_in(&s2));
        assert_eq!(record.scope(), "sandbox");
        Ok(())
    }
}
