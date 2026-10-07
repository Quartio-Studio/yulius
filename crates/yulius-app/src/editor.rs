//! The sandbox editor: access decisions and upstream resolution for the reverse proxy at
//! `/s/{sid}/editor/*` (SPEC-001 EDT-001/002, AC-EDT-1; SPEC-002 ADR-10).
//!
//! Only users with write access reach the editor. The default folder is the sandbox
//! repository; `?worktree=<task id>` redirects to the task's worktree (only worktrees of that
//! sandbox). The connection token stays server-side ([`EditorEndpoint::cookie`]).

use std::path::PathBuf;

use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::permission::Action;

use crate::auth::AuthContext;
use crate::browser::percent_encode;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::editor::{EditorEndpoint, EditorFailure};

/// Where an editor request goes.
#[derive(Debug, Clone)]
pub enum EditorRoute {
    /// Forward to the running editor.
    Proxy(EditorEndpoint),
    /// Send the browser elsewhere (worktree selection), path and query relative to the origin.
    Redirect(String),
}

/// Why an editor request is not served.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorDenied {
    /// Access or state failure, reported like any API error.
    App(AppError),
    /// openvscode-server is missing, disabled or failed to start: HTTP 503 with this message.
    Unavailable(String),
}

impl From<AppError> for EditorDenied {
    fn from(err: AppError) -> Self {
        Self::App(err)
    }
}

/// `/s/<sandbox>/editor`, the base path the editor serves under (docs/api.md).
#[must_use]
pub fn editor_base_path(sandbox: &SandboxId) -> String {
    format!("/s/{}/editor", sandbox.as_str())
}

impl App {
    /// Resolves an editor request for a write user: a redirect for `?worktree=<task id>`, else
    /// the upstream of the sandbox's editor (started on demand with the sandbox repository).
    ///
    /// # Errors
    /// [`EditorDenied::App`] for missing access (`NotFound`/`Forbidden`), unknown or foreign
    /// worktrees and an uncloned repository; [`EditorDenied::Unavailable`] when the editor
    /// cannot run.
    pub async fn editor_route(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        worktree: Option<&str>,
    ) -> Result<EditorRoute, EditorDenied> {
        let sandbox_id = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        let (record, _) = self.require(&ctx.user, &sandbox_id, Action::Work).await?;
        let base_path = editor_base_path(&record.id);
        if let Some(task) = worktree {
            let folder = self.sandbox_worktree(&record, task).await?;
            let folder = folder.to_string_lossy();
            return Ok(EditorRoute::Redirect(format!(
                "{base_path}/?folder={}",
                percent_encode(&folder, true)
            )));
        }
        let repo = self.sandbox_repo(&record.id);
        match self.deps.editor.ensure(&record.id, &repo, &base_path).await {
            Ok(endpoint) => Ok(EditorRoute::Proxy(endpoint)),
            Err(EditorFailure::InvalidWorkspace) => Err(EditorDenied::App(AppError::Conflict(
                "the sandbox repository is not cloned yet".into(),
            ))),
            Err(EditorFailure::Unavailable(message) | EditorFailure::Failed(message)) => {
                Err(EditorDenied::Unavailable(message))
            }
        }
    }

    /// Records editor activity (idle reaping).
    pub fn editor_touch(&self, sandbox: &str) {
        if let Ok(id) = SandboxId::parse(sandbox) {
            self.deps.editor.touch(&id);
        }
    }

    /// Whether the caller still has write access to the sandbox (open editor WebSockets are
    /// re-checked periodically, ACC-008).
    pub async fn editor_still_allowed(&self, ctx: &AuthContext, sandbox: &str) -> bool {
        let Ok(id) = SandboxId::parse(sandbox) else {
            return false;
        };
        ctx.user.disabled_at.is_none() && self.require(&ctx.user, &id, Action::Work).await.is_ok()
    }

    /// The worktree of `task` when it belongs to `sandbox` and lies inside the sandbox
    /// directory (shared by the editor and sandbox shells).
    ///
    /// # Errors
    /// `NotFound` for unknown tasks or tasks of other sandboxes, `Conflict` when the task has
    /// no worktree on disk.
    pub(crate) async fn sandbox_worktree(
        &self,
        sandbox: &SandboxRecord,
        task: &str,
    ) -> AppResult<PathBuf> {
        let task_id = TaskId::parse(task).map_err(|_| AppError::NotFound)?;
        let task = self
            .deps
            .store
            .task(&task_id)
            .await?
            .filter(|t| t.sandbox_id == sandbox.id)
            .ok_or(AppError::NotFound)?;
        let no_worktree = || AppError::Conflict("the task has no worktree yet".into());
        let path = task.worktree_path.ok_or_else(no_worktree)?;
        let worktree = std::fs::canonicalize(&path).map_err(|_| no_worktree())?;
        let root =
            std::fs::canonicalize(self.sandbox_dir(&sandbox.id)).map_err(|_| no_worktree())?;
        if !worktree.starts_with(&root) || !worktree.is_dir() {
            return Err(AppError::Conflict(
                "the task worktree is outside the sandbox".into(),
            ));
        }
        Ok(worktree)
    }
}
