//! A task's original prompt (SPEC-043, ADR-0033): the text it started with and the files attached
//! to it, read on demand by whoever may see the task.
//!
//! The prompt is stored as the composer sent it: the text, then one `@path` line per attached
//! file ([`Request::parse`] splits them, as the session start does). A line can name any path on
//! the host, so a file is only ever served from the task's own attachment folders: its creator's
//! composer drafts (`attachments/drafts/<user>/`) and the task's terminal attachments
//! (`attachments/tasks/<task>/`), named exactly as an upload reports them (no `..`, no other
//! folder). Those folders must be real folders, not links (a contained process, root in its
//! sandbox, can plant one); the file is opened without following a link or blocking on a pipe,
//! checked again through its handle (a regular file within the upload limit) and, where
//! `/proc/self/fd` exists, at the path it was opened at, so a folder swapped for a link between
//! the checks and the open is refused too. Reads share the uploads' concurrency bound.

use std::os::fd::AsRawFd as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Component, Path, PathBuf};

use tokio::io::AsyncReadExt as _;
use yulius_contracts::api::tasks::{PromptAttachment, TaskPrompt};
use yulius_domain::permission::Action;
use yulius_domain::request::{Request, is_image};

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::TaskRecord;
use crate::uploads::{ATTACHMENTS_DIR, MAX_ATTACHMENTS_PER_FOLDER, plain_dirs_exist};

/// Attached files of one prompt checked on disk; any further ones are listed as not available.
const MAX_CHECKED: usize = MAX_ATTACHMENTS_PER_FOLDER;

impl App {
    /// The prompt task `id` started with, its attached files listed by name.
    ///
    /// # Errors
    /// `NotFound` for unknown tasks or without read access.
    pub async fn task_prompt(&self, ctx: &AuthContext, id: &str) -> AppResult<TaskPrompt> {
        let task = self.load_task(ctx, id, Action::View).await?;
        let request = Request::parse(&task.prompt);
        let folders = self.owned_attachment_folders(&task);
        let attachments = request
            .attachments
            .iter()
            .enumerate()
            .map(|(i, path)| PromptAttachment {
                name: file_name(path),
                image: is_image(path),
                available: i < MAX_CHECKED
                    && served_path(&folders, path).is_some_and(|(file, _)| {
                        std::fs::symlink_metadata(file).is_ok_and(|meta| {
                            meta.file_type().is_file() && meta.len() <= self.config.upload_max_bytes
                        })
                    }),
            })
            .collect();
        Ok(TaskPrompt {
            text: request.text,
            attachments,
        })
    }

    /// Attachment `index` of task `id`'s prompt, read whole.
    ///
    /// # Errors
    /// `NotFound` for unknown tasks, without read access, and for an attachment out of range, gone
    /// or not served (not in the task's attachment folders, through a link, not a regular file,
    /// over the upload limit); `Capacity` while the uploads' bound is reached.
    pub async fn task_prompt_attachment(
        &self,
        ctx: &AuthContext,
        id: &str,
        index: usize,
    ) -> AppResult<AttachedFile> {
        let task = self.load_task(ctx, id, Action::View).await?;
        let request = Request::parse(&task.prompt);
        let path = request
            .attachments
            .get(index)
            .filter(|_| index < MAX_CHECKED)
            .ok_or(AppError::NotFound)?;
        let folders = self.owned_attachment_folders(&task);
        let (file, real) = served_path(&folders, path).ok_or(AppError::NotFound)?;
        let _slot = self.upload_slot()?;
        let limit = self.config.upload_max_bytes;
        let opened = open_attached(file, &real, limit).ok_or(AppError::NotFound)?;
        let mut bytes = Vec::new();
        tokio::fs::File::from_std(opened)
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

    /// The folders a file of `task`'s prompt may be in, as an upload names them (the sandbox
    /// directory as processes see it, and resolved), when they are real folders, not links; each
    /// with where it really is (below the resolved sandbox directory, which is trusted).
    fn owned_attachment_folders(&self, task: &TaskRecord) -> Vec<(PathBuf, PathBuf)> {
        let absolute = {
            let dir = self.sandbox_dir(&task.sandbox_id);
            std::path::absolute(&dir).unwrap_or(dir)
        };
        let Ok(resolved) = std::fs::canonicalize(&absolute) else {
            return Vec::new();
        };
        let mut bases = vec![absolute.clone()];
        if resolved != absolute {
            bases.push(resolved.clone());
        }
        let owners = [
            [ATTACHMENTS_DIR, "drafts", task.created_by.as_str()],
            [ATTACHMENTS_DIR, "tasks", task.id.as_str()],
        ];
        let mut folders = Vec::new();
        for base in &bases {
            for parts in &owners {
                if plain_dirs_exist(base, parts) {
                    let under =
                        |root: &PathBuf| parts.iter().fold(root.clone(), |p, part| p.join(part));
                    folders.push((under(base), under(&resolved)));
                }
            }
        }
        folders
    }
}

/// `path` when it names a file directly inside one of `folders` (no `.` or `..` on the way),
/// with where that file really is.
fn served_path<'a>(folders: &[(PathBuf, PathBuf)], path: &'a str) -> Option<(&'a Path, PathBuf)> {
    let file = Path::new(path);
    let plain = file
        .components()
        .all(|c| matches!(c, Component::RootDir | Component::Normal(_)));
    if !plain {
        return None;
    }
    let (parent, name) = (file.parent()?, file.file_name()?);
    let (_, real) = folders.iter().find(|(named, _)| named == parent)?;
    Some((file, real.join(name)))
}

/// Opens `file` without following a link or blocking on a pipe; `None` unless it is a regular
/// file within `limit` that was opened at `real` (where `/proc/self/fd` says).
fn open_attached(file: &Path, real: &Path, limit: u64) -> Option<std::fs::File> {
    use nix::fcntl::OFlag;
    let opened = std::fs::File::options()
        .read(true)
        .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
        .open(file)
        .ok()?;
    let meta = opened.metadata().ok()?;
    if !meta.is_file() || meta.len() > limit {
        return None;
    }
    // A folder on the way swapped for a link after the checks: the file opened is elsewhere.
    match std::fs::read_link(format!("/proc/self/fd/{}", opened.as_raw_fd())) {
        Ok(opened_at) => (opened_at == real).then_some(opened),
        Err(_) => Some(opened),
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
        let folders = [(
            PathBuf::from("/s/attachments/drafts/u"),
            PathBuf::from("/real/s/attachments/drafts/u"),
        )];
        assert_eq!(
            served_path(&folders, "/s/attachments/drafts/u/a.png").map(|(_, real)| real),
            Some(PathBuf::from("/real/s/attachments/drafts/u/a.png"))
        );
        for path in [
            "/s/attachments/drafts/u/../v/a.png",
            "/s/attachments/drafts/u/x/a.png",
            "/s/attachments/drafts/a.png",
            "/s/attachments/drafts/u",
        ] {
            assert!(served_path(&folders, path).is_none(), "{path}");
        }
    }
}
