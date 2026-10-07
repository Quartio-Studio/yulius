//! MCP server configuration (secrets are write-only).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// An MCP server as returned by the API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerView {
    /// Id.
    pub id: String,
    /// `instance` or `sandbox`.
    pub scope: String,
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
    /// Environment secrets (replace stored values for given keys).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Header secrets.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Enabled.
    #[serde(default)]
    pub enabled: Option<bool>,
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
