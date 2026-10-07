//! Personal VS Code (openvscode-server) port (SPEC-002 ADR-10; SPEC-001 EDT-001..004).
//!
//! The adapter starts one editor per (user, sandbox) on demand, listening on loopback and
//! requiring a connection token, inside the user's sandbox identity when contained. The
//! interface layer reverse-proxies `/s/{sid}/editor/*` to the caller's editor, adds
//! [`EditorEndpoint::cookie`] to every forwarded request and hands VS Code's client the token as
//! a cookie scoped to the editor path (its client must read it).

use std::path::PathBuf;

use async_trait::async_trait;

use crate::model::Secret;
use crate::ports::session::Containment;

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
    /// `Cookie` pair carrying the connection token (`vscode-tkn=<token>`).
    pub cookie: Secret,
}

/// Everything needed to start (or reuse) one user's editor in one sandbox (SPEC-001 EDT-003).
/// `*_arg` paths are as seen by the editor process; the others are host paths.
#[derive(Debug, Clone)]
pub struct EditorLaunch {
    /// One editor runs per key (`<sandbox>/<user>`).
    pub key: String,
    /// Host path of the persisted connection token.
    pub token_file: PathBuf,
    /// The token file as seen by the editor.
    pub token_file_arg: PathBuf,
    /// Host path of the user data directory (shared by the user's editors).
    pub user_data_dir: PathBuf,
    /// Host path of the extensions directory.
    pub extensions_dir: PathBuf,
    /// The user data directory as seen by the editor.
    pub user_data_arg: PathBuf,
    /// The extensions directory as seen by the editor.
    pub extensions_arg: PathBuf,
    /// Folder opened by default.
    pub workspace: PathBuf,
    /// Base path the editor serves under.
    pub base_path: String,
    /// Environment of the editor process.
    pub env: Vec<(String, String)>,
    /// Containment of the editor process (the user's sandbox identity when contained).
    pub containment: Containment,
}

/// Owns the running editors.
#[async_trait]
pub trait EditorPort: Send + Sync {
    /// The running editor of `launch.key`, started on demand. A running editor is reused as is.
    async fn ensure(&self, launch: &EditorLaunch) -> Result<EditorEndpoint, EditorFailure>;
    /// Records activity for idle tracking (per proxied request and while WebSockets are open).
    fn touch(&self, key: &str);
    /// Stops idle editors and reaps crashed ones; returns the stopped keys.
    async fn reap_idle(&self) -> Vec<String>;
    /// Stops every editor (server shutdown); returns keys whose process survived.
    async fn shutdown_all(&self) -> Vec<String>;
}
