//! Project variables (SPEC-275, ADR-0203): a project's non-secret values, shown and used in plain.

use serde::{Deserialize, Serialize};

/// A project variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectVariableView {
    /// NAME.
    pub name: String,
    /// The value, in plain.
    pub value: String,
    /// What it is.
    pub description: String,
    /// Who changed it last (login).
    pub updated_by: String,
    /// When.
    pub updated_at: String,
}

/// `GET /api/sandboxes/{sid}/variables`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectVariables {
    /// By name.
    pub variables: Vec<ProjectVariableView>,
    /// Whether the caller may set and delete them (write access).
    pub can_manage: bool,
}

/// One variable to set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectVariableInput {
    /// NAME (agent secret name rules).
    pub name: String,
    /// The value: one line, at most 4 KiB, may be empty.
    pub value: String,
    /// What it is; absent keeps the stored one, empty clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// `PUT /api/sandboxes/{sid}/variables` body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectVariablesSet {
    /// At most 200, each name once.
    pub variables: Vec<ProjectVariableInput>,
    /// Only create: a variable that exists is left as it is and answered as `existing`
    /// (`yulius secrets import`, which never overwrites).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub only_new: bool,
}

/// What a set did, by name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectVariablesSetResult {
    /// New.
    pub created: Vec<String>,
    /// Value or description changed.
    pub changed: Vec<String>,
    /// Already so.
    pub unchanged: Vec<String>,
    /// Left as they are (`only_new`).
    #[serde(default)]
    pub existing: Vec<String>,
}
