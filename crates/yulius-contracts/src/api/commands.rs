//! Brokered commands (SPEC-626, ADR-0459): a project's CLIs that need an agent secret as a file,
//! which agents run by name outside their sandbox.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

fn default_approve() -> String {
    "always".into()
}

const fn default_approval_minutes() -> u32 {
    10
}

const fn default_timeout_seconds() -> u32 {
    600
}

/// A command's definition (BRK-001), as an administrator writes it and as it is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandDefinition {
    /// 1 to 40 of `a-z`, `0-9` and `-`, unique in the project.
    pub name: String,
    /// What it is for.
    #[serde(default)]
    pub description: String,
    /// The binary's absolute path on the host's system.
    pub binary: String,
    /// The NAME of the agent secret its runs get as a file.
    pub secret: String,
    /// Task numbers that may run it; empty: every task of the project.
    #[serde(default)]
    pub tasks: Vec<u32>,
    /// `always`, `send` or `never`.
    #[serde(default = "default_approve")]
    pub approve: String,
    /// The flags that mark a run that changes something (`send`).
    #[serde(default)]
    pub send_flags: Vec<String>,
    /// How long a run waits for a person's answer (1 to 60).
    #[serde(default = "default_approval_minutes")]
    pub approval_minutes: u32,
    /// A run's time limit (1 to 1800).
    #[serde(default = "default_timeout_seconds")]
    pub timeout_seconds: u32,
    /// Arguments before the agent's; `{keyfile}` and `{home}` are replaced.
    #[serde(default)]
    pub fixed: Vec<String>,
    /// The allowed first non-flag arguments; empty: any.
    #[serde(default)]
    pub subcommands: Vec<String>,
    /// Flags Yulius sets itself and refuses from the agent.
    #[serde(default)]
    pub refuse: Vec<String>,
    /// Whether arguments like `owner/repo` pass (never an absolute path, `~` or `..`).
    #[serde(default)]
    pub slashes: bool,
    /// Environment variables; `{keyfile}`, `{home}` and `{secret}` are replaced.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// One command as its project's people see it (BRK-002, BRK-004).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandView {
    /// Its id.
    pub id: String,
    /// The definition.
    #[serde(flatten)]
    pub definition: CommandDefinition,
    /// Whether the secret it names reaches its runs now (a project or global agent secret
    /// switched on here, not a production one).
    pub secret_ready: bool,
    /// Who created it (login).
    pub created_by: String,
    /// When (RFC 3339).
    pub created_at: String,
    /// Who changed it last (login).
    pub updated_by: String,
    /// When (RFC 3339).
    pub updated_at: String,
    /// Whether the viewer may change it (an administrator of the project).
    pub can_edit: bool,
}

/// `GET /api/sandboxes/{sid}/commands`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandList {
    /// The project's commands, by name.
    pub items: Vec<CommandView>,
    /// Whether the viewer may add one.
    pub can_edit: bool,
}
