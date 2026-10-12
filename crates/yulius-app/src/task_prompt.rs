//! A task's original prompt (SPEC-043, ADR-0032): the text it started with and the files attached
//! to it, read on demand by whoever may see the task.
//!
//! The prompt is stored as the composer sent it: the text, then one `@path` line per attached
//! file ([`Request::parse`] splits them, as the session start does). A line can name any path on
//! the host, so a file is only ever served from the sandbox's attachments folder: its resolved
//! path must be a regular file inside it, no larger than the upload limit. Like the upload
//! checks, this is by path; a swap racing the check and the read is not excluded.

use std::path::{Path, PathBuf};

use tokio::io::AsyncReadExt as _;
use yulius_contracts::api::tasks::{PromptAttachment, TaskPrompt};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::request::{Request, is_image};

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
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

    /// Attachment `index` of task `id`'s prompt, read whole.
    ///
    /// # Errors
    /// `NotFound` for unknown tasks, without read access, and for an attachment out of range, gone
    /// or not served (outside the attachments folder, not a regular file, over the upload limit).
    pub async fn task_prompt_attachment(
        &self,
        ctx: &AuthContext,
        id: &str,
        index: usize,
    ) -> AppResult<AttachedFile> {
        let task = self.load_task(ctx, id, Action::View).await?;
        let request = Request::parse(&task.prompt);
        let path = request.attachments.get(index).ok_or(AppError::NotFound)?;
        let file = self
            .attached_file(&task.sandbox_id, path)
            .await
            .ok_or(AppError::NotFound)?;
        let mut bytes = Vec::new();
        let limit = self.config.upload_max_bytes;
        tokio::fs::File::open(&file)
            .await
            .map_err(|_| AppError::NotFound)?
            .take(limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| AppError::NotFound)?;
        // It grew past the limit since it was checked.
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limit {
            return Err(AppError::NotFound);
        }
        Ok(AttachedFile {
            name: file_name(path),
            content_type: image_type(path),
            bytes,
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

/// An attached file read for serving.
#[derive(Debug)]
pub struct AttachedFile {
    /// File name.
    pub name: String,
    /// Its image type when Claude Code takes it as an image; `None` for any other file.
    pub content_type: Option<&'static str>,
    /// Contents.
    pub bytes: Vec<u8>,
}

/// The image type of a file Claude Code takes as an image ([`is_image`]), by its extension.
fn image_type(path: &str) -> Option<&'static str> {
    let lower = path.to_ascii_lowercase();
    let ext = lower.rsplit_once('.')?.1;
    match ext {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// The last component of an attached file's path.
fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_image_claude_code_pastes_has_an_image_type() {
        for path in ["/a/b.png", "/a/b.JPG", "/a/b.jpeg", "/a/b.gif", "/a/b.webp"] {
            assert!(is_image(path));
            assert!(image_type(path).is_some(), "{path}");
        }
        assert_eq!(image_type("/a/b.svg"), None);
        assert_eq!(image_type("/a/png"), None);
        assert_eq!(file_name("/a/b c.png"), "b c.png");
    }
}
