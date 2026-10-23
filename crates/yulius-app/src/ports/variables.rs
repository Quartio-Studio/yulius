//! Project variables persistence port (SPEC-275, ADR-0203): a project's non-secret values, kept
//! in plain, one row per sandbox and name.

use async_trait::async_trait;
use yulius_domain::ids::SandboxId;
use yulius_domain::time::Timestamp;

use crate::error::AppResult;

/// A project variable as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectVariableRecord {
    /// Its project.
    pub sandbox_id: SandboxId,
    /// Its name (an agent secret name).
    pub name: String,
    /// Its value, in plain.
    pub value: String,
    /// What it is.
    pub description: String,
    /// Who created it (audit actor).
    pub created_by: String,
    /// When.
    pub created_at: Timestamp,
    /// Who changed it last.
    pub updated_by: String,
    /// When.
    pub updated_at: Timestamp,
}

/// One variable to write: an absent description keeps the stored one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableWrite {
    /// Its name.
    pub name: String,
    /// Its value.
    pub value: String,
    /// Its description; `None` keeps the stored one (empty for a new variable).
    pub description: Option<String>,
}

/// What writing one variable did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariableOutcome {
    /// It is new.
    Created,
    /// These fields changed (`value`, `description`).
    Changed(Vec<&'static str>),
    /// It was already so.
    Unchanged,
    /// It exists and the write was for new variables only: left as it is.
    Existing,
}

/// Project variable persistence (SPEC-275 VAR-001).
#[async_trait]
pub trait ProjectVariableStore: Send + Sync {
    /// The variables of `sandbox`, by name.
    async fn project_variables(&self, sandbox: &SandboxId)
    -> AppResult<Vec<ProjectVariableRecord>>;
    /// Writes `writes` of `sandbox` in one transaction as `by` at `at`, deciding each outcome
    /// against the rows as they are inside it: a new name is inserted, an existing one changed
    /// (keeping who created it) unless `only_new`. Nothing is written, and `None` answered, when
    /// the project would then hold more than `cap`.
    async fn project_variables_put(
        &self,
        sandbox: &SandboxId,
        writes: Vec<VariableWrite>,
        (by, at): (&str, Timestamp),
        only_new: bool,
        cap: usize,
    ) -> AppResult<Option<Vec<(String, VariableOutcome)>>>;
    /// Deletes the variable `name` of `sandbox`; `false` when there was none.
    async fn project_variable_delete(&self, sandbox: &SandboxId, name: &str) -> AppResult<bool>;
}
