//! [`EditorPort`] over `yulius-editor`: openvscode-server per sandbox on demand (SPEC-002
//! ADR-10).

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use yulius_app::model::Secret;
use yulius_app::ports::editor::{EditorEndpoint, EditorFailure, EditorPort};
use yulius_domain::ids::SandboxId;
use yulius_editor::{EditorConfig, EditorError, EditorManager};

/// openvscode-server editors per sandbox.
pub struct VsCodeEditors {
    manager: Arc<EditorManager>,
}

impl VsCodeEditors {
    /// Editors started with `config`.
    #[must_use]
    pub fn new(config: EditorConfig) -> Self {
        Self {
            manager: Arc::new(EditorManager::new(config)),
        }
    }
}

fn failure(error: EditorError) -> EditorFailure {
    match error {
        EditorError::ExecutableNotFound { .. } => EditorFailure::Unavailable(error.to_string()),
        EditorError::InvalidWorkspace(_) => EditorFailure::InvalidWorkspace,
        other => EditorFailure::Failed(format!("the editor could not be started: {other}")),
    }
}

#[async_trait]
impl EditorPort for VsCodeEditors {
    async fn ensure(
        &self,
        sandbox: &SandboxId,
        workspace: &Path,
        base_path: &str,
    ) -> Result<EditorEndpoint, EditorFailure> {
        let instance = self
            .manager
            .ensure(sandbox, workspace, base_path)
            .await
            .map_err(failure)?;
        Ok(EditorEndpoint {
            port: instance.port(),
            base_path: instance.base_path().to_owned(),
            cookie: Secret(instance.connection_token().cookie_value()),
        })
    }

    fn touch(&self, sandbox: &SandboxId) {
        self.manager.touch(sandbox);
    }

    async fn reap_idle(&self) -> Vec<SandboxId> {
        self.manager.reap_idle(Instant::now()).await
    }

    async fn shutdown_all(&self) -> Vec<SandboxId> {
        self.manager.stop_all().await
    }
}

/// Placeholder when the editor is not configured or disabled: every request gets a clear
/// "unavailable" message (HTTP 503).
pub struct UnavailableEditor {
    reason: String,
}

impl UnavailableEditor {
    /// A placeholder reporting `reason`.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl Default for UnavailableEditor {
    fn default() -> Self {
        Self::new("the editor is not available in this build")
    }
}

#[async_trait]
impl EditorPort for UnavailableEditor {
    async fn ensure(
        &self,
        _: &SandboxId,
        _: &Path,
        _: &str,
    ) -> Result<EditorEndpoint, EditorFailure> {
        Err(EditorFailure::Unavailable(self.reason.clone()))
    }

    fn touch(&self, _: &SandboxId) {}

    async fn reap_idle(&self) -> Vec<SandboxId> {
        Vec::new()
    }

    async fn shutdown_all(&self) -> Vec<SandboxId> {
        Vec::new()
    }
}
