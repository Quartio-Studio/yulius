//! Skills catalog.

use serde::{Deserialize, Serialize};

/// A skill visible in a sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillView {
    /// Skill name.
    pub name: String,
    /// Description used for automatic activation.
    pub description: String,
    /// `repository`, `instance` or `open-mercato`.
    pub source: String,
    /// Location (repository-relative or catalog path).
    pub path: String,
    /// Installed into harness homes for this sandbox.
    pub enabled: bool,
}

/// Skill content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillContent {
    /// Skill name.
    pub name: String,
    /// `SKILL.md` text.
    pub content: String,
}

/// `PUT /api/sandboxes/{sid}/skills/{name}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetSkillEnabled {
    /// Enabled flag.
    pub enabled: bool,
}
