//! MCP server configuration (secrets are write-only).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// An MCP server as returned by the API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerView {
    /// Id.
    pub id: String,
    /// `instance` (every sandbox) or `sandbox` (this sandbox).
    pub scope: String,
    /// `team` (every run of its scope) or `personal` (only its owner's runs, SPEC-332); listed
    /// only to its owner.
    pub audience: String,
    /// Name (also the tool namespace).
    pub name: String,
    /// `stdio` or `http`.
    pub transport: String,
    /// Executable for stdio servers.
    pub command: Option<String>,
    /// Arguments for stdio servers.
    pub args: Vec<String>,
    /// URL for HTTP servers.
    pub url: Option<String>,
    /// Names of stored environment secrets.
    pub env_keys: Vec<String>,
    /// Names of stored header secrets.
    pub header_keys: Vec<String>,
    /// Enabled.
    pub enabled: bool,
    /// `person` (a person added it or changed what it runs) or `mcp_json` (imported from the
    /// repository's `.mcp.json`, which agents can write: the Test does not run its command).
    pub origin: String,
}

/// Create/update body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerInput {
    /// Name.
    pub name: String,
    /// `stdio` or `http`.
    pub transport: String,
    /// Executable for stdio servers.
    #[serde(default)]
    pub command: Option<String>,
    /// Arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// URL for HTTP servers.
    #[serde(default)]
    pub url: Option<String>,
    /// Environment secrets (stdio). Omitted on update: keep the stored values; given: replace
    /// all stored values (an empty object clears them).
    #[serde(default)]
    pub env: Option<BTreeMap<String, String>>,
    /// Header secrets (HTTP), with the same update rule as `env`.
    #[serde(default)]
    pub headers: Option<BTreeMap<String, String>>,
    /// Enabled (create: default true; update: default unchanged).
    #[serde(default)]
    pub enabled: Option<bool>,
    /// `instance` (every sandbox) or `sandbox` (create: default `sandbox`; update: default
    /// unchanged). A team server for every sandbox needs an instance administrator.
    #[serde(default)]
    pub scope: Option<String>,
    /// `team` or `personal` (create: default `team`; update: default unchanged). Making a server
    /// personal makes the caller its owner (SPEC-332).
    #[serde(default)]
    pub audience: Option<String>,
    /// Update only: the sandbox a server moved from `instance` to `sandbox` scope belongs to.
    #[serde(default)]
    pub sandbox: Option<String>,
}

/// `POST /api/mcp/{id}/test`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpTestResult {
    /// Connected and listed tools.
    pub ok: bool,
    /// Tool names.
    pub tools: Vec<String>,
    /// Error text.
    pub error: Option<String>,
}
