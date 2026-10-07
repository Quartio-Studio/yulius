//! Per-sandbox VS Code (openvscode-server) port (SPEC-002 ADR-10; SPEC-001 EDT-001/002).
//!
//! The adapter starts one editor per sandbox on demand, listening on loopback and requiring a
//! connection token. The interface layer reverse-proxies `/s/{sid}/editor/*` to
//! [`EditorEndpoint::port`] and adds [`EditorEndpoint::cookie`] to every forwarded request; the
//! token never reaches the browser.

use std::path::Path;

use async_trait::async_trait;
use yulius_domain::ids::SandboxId;

use crate::model::Secret;

/// Why the editor is not reachable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditorFailure {
    /// openvscode-server is not installed, not configured or disabled by configuration.
    #[error("{0}")]
    Unavailable(String),
    /// The workspace folder does not exist (e.g. the repository is not cloned yet).
    #[error("the editor workspace folder does not exist")]
    InvalidWorkspace,
    /// Starting or health-checking the editor failed.
    #[error("{0}")]
    Failed(String),
}

/// How to reach a running editor.
#[derive(Debug, Clone)]
pub struct EditorEndpoint {
    /// Loopback port of the editor.
    pub port: u16,
    /// Base path the editor serves under (`/s/<sandbox>/editor`).
    pub base_path: String,
    /// `Cookie` pair carrying the connection token (`vscode-tkn=<token>`); send only upstream.
    pub cookie: Secret,
}

/// Owns at most one editor per sandbox.
#[async_trait]
pub trait EditorPort: Send + Sync {
    /// The sandbox's running editor, started on demand with `workspace` as its default folder
    /// and serving under `base_path`. A running editor is reused as is.
    async fn ensure(
        &self,
        sandbox: &SandboxId,
        workspace: &Path,
        base_path: &str,
    ) -> Result<EditorEndpoint, EditorFailure>;
    /// Records activity for idle tracking (per proxied request and while WebSockets are open).
    fn touch(&self, sandbox: &SandboxId);
    /// Stops idle editors and reaps crashed ones; returns the stopped sandboxes.
    async fn reap_idle(&self) -> Vec<SandboxId>;
    /// Stops every editor (server shutdown); returns sandboxes whose process survived.
    async fn shutdown_all(&self) -> Vec<SandboxId>;
}
