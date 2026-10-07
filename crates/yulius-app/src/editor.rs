//! The sandbox editor: access decisions and upstream resolution for the reverse proxy at
//! `/s/{sid}/editor/*` (SPEC-001 EDT-001/002, AC-EDT-1; SPEC-002 ADR-10).
//!
//! Only users with write access reach the editor; every user gets their own editor per sandbox
//! (profile and extensions in their home, contained as their sandbox identity; EDT-003). The
//! default folder is the sandbox repository; `?worktree=<task id>` redirects to the task's
//! worktree (only worktrees of that sandbox).

use std::path::PathBuf;

use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::permission::Action;

use crate::auth::AuthContext;
use crate::browser::percent_encode;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, UserRecord};
use crate::ports::editor::{EditorEndpoint, EditorFailure, EditorLaunch};
use crate::ports::session::{Bind, Containment};

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

/// Where a contained editor reads its connection token.
const EDITOR_TOKEN_PATH: &str = "/yulius/editor-token";

/// The editor slot of `user` in `sandbox`.
#[must_use]
pub fn editor_key(sandbox: &SandboxId, user: &UserId) -> String {
    format!("{}/{}", sandbox.as_str(), user.as_str())
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
        if !repo.is_dir() {
            return Err(EditorDenied::App(AppError::Conflict(
                "the sandbox repository is not cloned yet".into(),
            )));
        }
        let launch = self.editor_launch(&ctx.user, &record, repo, base_path)?;
        match self.deps.editor.ensure(&launch).await {
            Ok(endpoint) => Ok(EditorRoute::Proxy(endpoint)),
            Err(EditorFailure::InvalidWorkspace) => Err(EditorDenied::App(AppError::Conflict(
                "the sandbox repository is not cloned yet".into(),
            ))),
            Err(EditorFailure::Unavailable(message) | EditorFailure::Failed(message)) => {
                Err(EditorDenied::Unavailable(message))
            }
        }
    }

    /// The launch of `user`'s editor in `sandbox`: profile and extensions in the user's home
    /// (shared by all their sandboxes), the token under `editors/<sandbox>/<user>/`, and the
    /// user's sandbox identity when contained (SPEC-002 ADR-10/12).
    fn editor_launch(
        &self,
        user: &UserRecord,
        sandbox: &SandboxRecord,
        workspace: PathBuf,
        base_path: String,
    ) -> AppResult<EditorLaunch> {
        let space = self.user_space(user, sandbox)?;
        let token_file = self
            .config
            .data_dir
            .join("editors")
            .join(sandbox.id.as_str())
            .join(user.id.as_str())
            .join("connection-token");
        let (user_data_arg, user_data_dir) = space.home_path(".yulius/vscode/data");
        let (extensions_arg, extensions_dir) = space.home_path(".yulius/vscode/extensions");
        let uncontained_env = self.uncontained_editor_env(&space);
        let (containment, token_file_arg) =
            match (space.containment, self.config.editor_uncontained) {
                (
                    Containment::Bubblewrap {
                        sandbox_root,
                        home,
                        writable,
                        readonly,
                        mut binds,
                        identity,
                    },
                    false,
                ) => {
                    let arg = PathBuf::from(EDITOR_TOKEN_PATH);
                    binds.push(Bind {
                        source: token_file.clone(),
                        dest: arg.clone(),
                        writable: false,
                    });
                    (
                        Containment::Bubblewrap {
                            sandbox_root,
                            home,
                            writable,
                            readonly,
                            binds,
                            identity,
                        },
                        arg,
                    )
                }
                // Uncontained (development, or `--editor-uncontained`): host paths throughout.
                _ => {
                    return Ok(EditorLaunch {
                        key: editor_key(&sandbox.id, &user.id),
                        token_file_arg: token_file.clone(),
                        token_file,
                        user_data_arg: user_data_dir.clone(),
                        extensions_arg: extensions_dir.clone(),
                        user_data_dir,
                        extensions_dir,
                        workspace,
                        base_path,
                        env: uncontained_env,
                        containment: Containment::None,
                    });
                }
            };
        Ok(EditorLaunch {
            key: editor_key(&sandbox.id, &user.id),
            token_file,
            token_file_arg,
            user_data_dir,
            extensions_dir,
            user_data_arg,
            extensions_arg,
            workspace,
            base_path,
            env: space.env,
            containment,
        })
    }

    /// The editor environment without containment: the user's variables with the home's host
    /// path and the host `PATH`.
    fn uncontained_editor_env(&self, space: &crate::identity::UserSpace) -> Vec<(String, String)> {
        let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
        space
            .env
            .iter()
            .filter(|(k, _)| k != "HOME" && k != "PATH")
            .cloned()
            .chain([
                (
                    "HOME".to_owned(),
                    space.home_host.to_string_lossy().into_owned(),
                ),
                ("PATH".to_owned(), path),
            ])
            .collect()
    }

    /// Records activity of the caller's editor (idle reaping).
    pub fn editor_touch(&self, ctx: &AuthContext, sandbox: &str) {
        if let Ok(id) = SandboxId::parse(sandbox) {
            self.deps.editor.touch(&editor_key(&id, &ctx.user.id));
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
