//! [`EditorPort`] over `yulius-editor`: openvscode-server per (user, sandbox) on demand, wrapped
//! in bubblewrap with the user's sandbox identity when contained (SPEC-002 ADR-10/12).

use std::ffi::OsString;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use yulius_app::model::Secret;
use yulius_app::ports::editor::{EditorEndpoint, EditorFailure, EditorLaunch, EditorPort};
use yulius_contracts::ptyhost::Containment;
use yulius_editor::{EditorConfig, EditorError, EditorManager, EditorSpec};
use yulius_ptyhost::containment::bwrap_argv;

use super::session::wire_containment;

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

/// The editor spec of a launch: the containment becomes a bubblewrap command prefix.
fn spec_of(launch: &EditorLaunch) -> EditorSpec {
    let wrapper: Vec<OsString> = match wire_containment(&launch.containment) {
        Containment::None => Vec::new(),
        Containment::Bubblewrap(spec) => {
            bwrap_argv(&spec, &[], &launch.workspace.to_string_lossy())
                .into_iter()
                .map(OsString::from)
                .collect()
        }
    };
    EditorSpec {
        key: launch.key.clone(),
        token_file: launch.token_file.clone(),
        token_file_arg: launch.token_file_arg.clone(),
        user_data_dir: launch.user_data_dir.clone(),
        extensions_dir: launch.extensions_dir.clone(),
        user_data_arg: launch.user_data_arg.clone(),
        extensions_arg: launch.extensions_arg.clone(),
        workspace_dir: launch.workspace.clone(),
        base_path: launch.base_path.clone(),
        wrapper,
        env: launch
            .env
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect(),
    }
}

#[async_trait]
impl EditorPort for VsCodeEditors {
    async fn ensure(&self, launch: &EditorLaunch) -> Result<EditorEndpoint, EditorFailure> {
        let instance = self
            .manager
            .ensure_spec(&spec_of(launch))
            .await
            .map_err(failure)?;
        Ok(EditorEndpoint {
            port: instance.port(),
            base_path: instance.base_path().to_owned(),
            cookie: Secret(instance.connection_token().cookie_value()),
        })
    }

    fn touch(&self, key: &str) {
        self.manager.touch_key(key);
    }

    async fn reap_idle(&self) -> Vec<String> {
        self.manager.reap_idle(Instant::now()).await
    }

    async fn shutdown_all(&self) -> Vec<String> {
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
    async fn ensure(&self, _: &EditorLaunch) -> Result<EditorEndpoint, EditorFailure> {
        Err(EditorFailure::Unavailable(self.reason.clone()))
    }

    fn touch(&self, _: &str) {}

    async fn reap_idle(&self) -> Vec<String> {
        Vec::new()
    }

    async fn shutdown_all(&self) -> Vec<String> {
        Vec::new()
    }
}
