//! A task's original prompt (SPEC-043, ADR-0032): the text it started with and the files attached
//! to it, read on demand by whoever may see the task.
//!
//! The prompt is stored as the composer sent it: the text, then one `@path` line per attached
//! file ([`Request::parse`] splits them, as the session start does). A line can name any path on
//! the host, so a file is only ever served from the sandbox's attachments folder: its resolved
//! path must be a regular file inside it, no larger than the upload limit. Like the upload
//! checks, this is by path; a swap racing the check and the read is not excluded.

use std::path::{Path, PathBuf};

use yulius_contracts::api::tasks::{PromptAttachment, TaskPrompt};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::request::{Request, is_image};

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::AppResult;
use crate::uploads::ATTACHMENTS_DIR;

impl App {
    /// The prompt task `id` started with, its attached files listed by name.
    ///
    /// # Errors
    /// `NotFound` for unknown tasks or without read access.
    pub async fn task_prompt(&self, ctx: &AuthContext, id: &str) -> AppResult<TaskPrompt> {
        let task = self.load_task(ctx, id, Action::View).await?;
        let request = Request::parse(&task.prompt);
        let mut attachments = Vec::with_capacity(request.attachments.len());
        for path in &request.attachments {
            attachments.push(PromptAttachment {
                name: file_name(path),
                image: is_image(path),
                available: self.attached_file(&task.sandbox_id, path).await.is_some(),
            });
        }
        Ok(TaskPrompt {
            text: request.text,
            attachments,
        })
    }

    /// `path` resolved, when it is a regular file inside the sandbox's attachments folder and
    /// within the upload limit; `None` otherwise.
    async fn attached_file(&self, sandbox: &SandboxId, path: &str) -> Option<PathBuf> {
        let root = tokio::fs::canonicalize(self.sandbox_dir(sandbox).join(ATTACHMENTS_DIR))
            .await
            .ok()?;
        let file = tokio::fs::canonicalize(path).await.ok()?;
        if !file.starts_with(&root) {
            return None;
        }
        let meta = tokio::fs::metadata(&file).await.ok()?;
        (meta.is_file() && meta.len() <= self.config.upload_max_bytes).then_some(file)
    }
}

/// The last component of an attached file's path.
fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}
