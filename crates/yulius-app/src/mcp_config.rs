//! MCP servers attached to agent runs (SPEC-001 MCP-001..003). The Yulius server itself is
//! always attached by the harness adapter; configured servers are added in CP7.

use crate::context::App;
use crate::error::AppResult;
use crate::model::SandboxRecord;
use crate::ports::harness::McpAttachment;

impl App {
    /// Configured MCP servers for runs in `sandbox` (instance scope plus sandbox scope).
    ///
    /// # Errors
    /// Store or crypto failures.
    pub async fn mcp_attachments(&self, _sandbox: &SandboxRecord) -> AppResult<Vec<McpAttachment>> {
        Ok(Vec::new())
    }
}
