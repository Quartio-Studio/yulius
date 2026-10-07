//! Workflows: reusable task recipes.

use serde::{Deserialize, Serialize};

/// Gates a workflow enforces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WorkflowGates {
    /// A valid spec is required before delivery.
    pub require_spec: bool,
    /// The project test command must pass before delivery.
    pub require_tests: bool,
}

/// A workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workflow {
    /// Id (ignored on create).
    #[serde(default)]
    pub id: String,
    /// Name.
    pub name: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Initial step titles.
    #[serde(default)]
    pub steps: Vec<String>,
    /// Extra instructions given to the agent.
    #[serde(default)]
    pub instructions: String,
    /// Gates.
    #[serde(default)]
    pub gates: WorkflowGates,
    /// `instance` or `sandbox` (read-only).
    #[serde(default)]
    pub scope: String,
}
