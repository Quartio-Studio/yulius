//! Sandbox files for the built-in editor (SPEC-001 EDT-001..004).
//!
//! Contract:
//! - only users with write access to the sandbox list, read or change files;
//! - the root is the sandbox repository or a task worktree of that sandbox; paths are
//!   relative, made of normal components only, and every existing path (symbolic links
//!   resolved) must stay inside the root;
//! - `.git` directories are not listed and cannot be read or changed through this API;
//! - reads return UTF-8 text up to [`MAX_FILE_BYTES`] (binary or larger files are reported, not
//!   returned) with the SHA-256 of the bytes on disk;
//! - writes are atomic (temporary file + rename, permissions kept) and refused with `Conflict`
//!   when the file changed since the editor read it (agents edit the same files) or, for a new
//!   file, when it already exists;
//! - each user's editor view (open files, unsaved text) lives only in their browser; nothing
//!   of it is stored or shared.

use std::path::{Component, Path, PathBuf};

use yulius_contracts::api::files::{
    CreateEntry, DirListing, FileContent, FileEntry, FileWritten, WriteFile,
};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::permission::Action;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;

/// Largest file the editor opens or writes.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Entries returned per directory listing.
pub const MAX_ENTRIES: usize = 2000;
/// Bytes inspected for NUL when telling text from binary.
const SNIFF_BYTES: usize = 8192;

/// A resolved file request: the root folder and the path below it.
struct Target {
    root: PathBuf,
    path: PathBuf,
    relative: String,
}

/// Normalizes a relative path: normal components only, no `.git`.
fn relative_components(path: &str) -> AppResult<Vec<String>> {
    if path.contains('\0') || path.starts_with('/') {
        return Err(AppError::Invalid("paths are relative to the folder".into()));
    }
    let mut parts = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy().into_owned();
                if part == ".git" {
                    return Err(AppError::Forbidden(
                        "the .git directory is not editable".into(),
                    ));
                }
                parts.push(part);
            }
            Component::CurDir => {}
            _ => return Err(AppError::Invalid("invalid path".into())),
        }
    }
    Ok(parts)
}

fn io_error(err: &std::io::Error) -> AppError {
    match err.kind() {
        std::io::ErrorKind::NotFound => AppError::NotFound,
        std::io::ErrorKind::PermissionDenied => AppError::Forbidden("permission denied".into()),
        _ => AppError::internal(err.to_string()),
    }
}

/// Whether `path` (resolved) stays inside `root` (resolved).
fn inside(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

impl App {
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

    /// Resolves `root` and `path` for a write user of `sandbox`.
    async fn file_target(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        root: Option<&str>,
        path: &str,
    ) -> AppResult<Target> {
        let sandbox_id = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        let (record, _) = self.require(&ctx.user, &sandbox_id, Action::Work).await?;
        let folder = match root.unwrap_or("sandbox") {
            "sandbox" | "" => {
                let repo = self.sandbox_repo(&record.id);
                if !repo.is_dir() {
                    return Err(AppError::Conflict(
                        "the sandbox repository is not cloned yet".into(),
                    ));
                }
                repo
            }
            other => {
                let task = other
                    .strip_prefix("task:")
                    .ok_or_else(|| AppError::Invalid("root is sandbox or task:<id>".into()))?;
                self.sandbox_worktree(&record, task).await?
            }
        };
        let root = std::fs::canonicalize(&folder).map_err(|e| io_error(&e))?;
        let parts = relative_components(path)?;
        let relative = parts.join("/");
        let path = parts.iter().fold(root.clone(), |p, part| p.join(part));
        Ok(Target {
            root,
            path,
            relative,
        })
    }

    /// Resolves an existing target and checks it stays inside the root.
    fn existing(target: &Target) -> AppResult<PathBuf> {
        let resolved = std::fs::canonicalize(&target.path).map_err(|e| io_error(&e))?;
        if !inside(&target.root, &resolved) {
            return Err(AppError::Forbidden(
                "the path leads outside the folder".into(),
            ));
        }
        Ok(resolved)
    }

    /// Lists a directory.
    ///
    /// # Errors
    /// Access errors, `NotFound`, `Invalid` paths, `Forbidden` outside the folder.
    pub async fn files_list(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        root: Option<&str>,
        path: &str,
    ) -> AppResult<DirListing> {
        let target = self.file_target(ctx, sandbox, root, path).await?;
        let dir = Self::existing(&target)?;
        let mut entries = Vec::new();
        let mut truncated = false;
        let reader = std::fs::read_dir(&dir).map_err(|e| io_error(&e))?;
        for entry in reader.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                continue;
            }
            if entries.len() >= MAX_ENTRIES {
                truncated = true;
                break;
            }
            let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
                continue;
            };
            let kind = if meta.file_type().is_symlink() {
                // Links into the folder behave like their target; others are shown as links.
                match std::fs::canonicalize(entry.path()) {
                    Ok(resolved) if inside(&target.root, &resolved) && resolved.is_dir() => "dir",
                    Ok(resolved) if inside(&target.root, &resolved) => "file",
                    _ => "symlink",
                }
            } else if meta.is_dir() {
                "dir"
            } else {
                "file"
            };
            let relative = if target.relative.is_empty() {
                name.clone()
            } else {
                format!("{}/{name}", target.relative)
            };
            entries.push(FileEntry {
                name,
                path: relative,
                kind: kind.into(),
                size: if kind == "file" { meta.len() } else { 0 },
            });
        }
        entries.sort_by(|a, b| {
            (a.kind != "dir")
                .cmp(&(b.kind != "dir"))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(DirListing {
            path: target.relative,
            entries,
            truncated,
        })
    }

    /// Reads a file for the editor.
    ///
    /// # Errors
    /// As [`App::files_list`]; `Invalid` for directories.
    pub async fn file_read(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        root: Option<&str>,
        path: &str,
    ) -> AppResult<FileContent> {
        let target = self.file_target(ctx, sandbox, root, path).await?;
        let file = Self::existing(&target)?;
        let meta = std::fs::metadata(&file).map_err(|e| io_error(&e))?;
        if meta.is_dir() {
            return Err(AppError::Invalid("that path is a directory".into()));
        }
        if meta.len() > MAX_FILE_BYTES {
            return Ok(FileContent {
                path: target.relative,
                content: None,
                binary: false,
                too_large: true,
                size: meta.len(),
                sha256: String::new(),
            });
        }
        let bytes = std::fs::read(&file).map_err(|e| io_error(&e))?;
        let sha256 = self.deps.crypto.sha256_hex(&bytes);
        let sniff = bytes
            .get(..SNIFF_BYTES.min(bytes.len()))
            .unwrap_or_default();
        let text = if sniff.contains(&0) {
            None
        } else {
            String::from_utf8(bytes).ok()
        };
        Ok(FileContent {
            path: target.relative,
            binary: text.is_none(),
            content: text,
            too_large: false,
            size: meta.len(),
            sha256,
        })
    }

    /// Writes a file atomically if it did not change since `base_sha256` (or creates it when
    /// the base is absent).
    ///
    /// # Errors
    /// As [`App::files_list`]; `Conflict` when the file changed or already exists; `Invalid`
    /// for oversized content.
    pub async fn file_write(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        req: &WriteFile,
    ) -> AppResult<FileWritten> {
        if req.path.trim().is_empty() {
            return Err(AppError::Invalid("a file path is required".into()));
        }
        if u64::try_from(req.content.len()).unwrap_or(u64::MAX) > MAX_FILE_BYTES {
            return Err(AppError::Invalid(format!(
                "files are limited to {MAX_FILE_BYTES} bytes in the editor"
            )));
        }
        let target = self
            .file_target(ctx, sandbox, req.root.as_deref(), &req.path)
            .await?;
        let parent = target
            .path
            .parent()
            .ok_or_else(|| AppError::Invalid("invalid path".into()))?;
        let parent = std::fs::canonicalize(parent).map_err(|e| io_error(&e))?;
        if !inside(&target.root, &parent) {
            return Err(AppError::Forbidden(
                "the path leads outside the folder".into(),
            ));
        }
        let file = parent.join(target.path.file_name().unwrap_or_default());
        let existing = match std::fs::symlink_metadata(&file) {
            Ok(_) => Some(Self::existing(&target)?),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(io_error(&err)),
        };
        let mode = match (&existing, &req.base_sha256) {
            (Some(path), Some(base)) => {
                let current = std::fs::read(path).map_err(|e| io_error(&e))?;
                if &self.deps.crypto.sha256_hex(&current) != base {
                    return Err(AppError::Conflict(
                        "the file changed on disk since you opened it; reload it first".into(),
                    ));
                }
                std::fs::metadata(path).ok().map(|m| m.permissions())
            }
            (Some(_), None) => {
                return Err(AppError::Conflict("a file with that name exists".into()));
            }
            (None, Some(_)) => {
                return Err(AppError::Conflict(
                    "the file was deleted on disk since you opened it".into(),
                ));
            }
            (None, None) => None,
        };
        let destination = existing.unwrap_or(file);
        let temporary = destination.with_file_name(format!(
            ".{}.yulius-edit",
            destination
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
        ));
        std::fs::write(&temporary, req.content.as_bytes()).map_err(|e| io_error(&e))?;
        if let Some(permissions) = mode {
            let _ = std::fs::set_permissions(&temporary, permissions);
        }
        if let Err(err) = std::fs::rename(&temporary, &destination) {
            let _ = std::fs::remove_file(&temporary);
            return Err(io_error(&err));
        }
        Ok(FileWritten {
            path: target.relative,
            sha256: self.deps.crypto.sha256_hex(req.content.as_bytes()),
            size: u64::try_from(req.content.len()).unwrap_or(u64::MAX),
        })
    }

    /// Creates an empty file or a directory.
    ///
    /// # Errors
    /// As [`App::files_list`]; `Conflict` when the path exists.
    pub async fn file_create(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        req: &CreateEntry,
    ) -> AppResult<FileEntry> {
        if req.kind == "file" {
            let written = self
                .file_write(
                    ctx,
                    sandbox,
                    &WriteFile {
                        root: req.root.clone(),
                        path: req.path.clone(),
                        content: String::new(),
                        base_sha256: None,
                    },
                )
                .await?;
            let name = written
                .path
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_owned();
            return Ok(FileEntry {
                name,
                path: written.path,
                kind: "file".into(),
                size: 0,
            });
        }
        if req.kind != "dir" {
            return Err(AppError::Invalid("kind is file or dir".into()));
        }
        let target = self
            .file_target(ctx, sandbox, req.root.as_deref(), &req.path)
            .await?;
        if target.relative.is_empty() {
            return Err(AppError::Invalid("a directory name is required".into()));
        }
        if std::fs::symlink_metadata(&target.path).is_ok() {
            return Err(AppError::Conflict("that path exists".into()));
        }
        let parent = target
            .path
            .parent()
            .map(std::fs::canonicalize)
            .transpose()
            .map_err(|e| io_error(&e))?
            .ok_or_else(|| AppError::Invalid("invalid path".into()))?;
        if !inside(&target.root, &parent) {
            return Err(AppError::Forbidden(
                "the path leads outside the folder".into(),
            ));
        }
        std::fs::create_dir(&target.path).map_err(|e| io_error(&e))?;
        let name = target
            .relative
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned();
        Ok(FileEntry {
            name,
            path: target.relative,
            kind: "dir".into(),
            size: 0,
        })
    }

    /// Deletes a file, a symbolic link or a directory tree (never the folder itself).
    ///
    /// # Errors
    /// As [`App::files_list`].
    pub async fn file_delete(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        root: Option<&str>,
        path: &str,
    ) -> AppResult<()> {
        let target = self.file_target(ctx, sandbox, root, path).await?;
        if target.relative.is_empty() {
            return Err(AppError::Invalid(
                "the folder itself cannot be deleted".into(),
            ));
        }
        let meta = std::fs::symlink_metadata(&target.path).map_err(|e| io_error(&e))?;
        // The entry itself must be inside the folder (its parent resolved), even for links.
        let parent = target
            .path
            .parent()
            .map(std::fs::canonicalize)
            .transpose()
            .map_err(|e| io_error(&e))?
            .ok_or_else(|| AppError::Invalid("invalid path".into()))?;
        if !inside(&target.root, &parent) {
            return Err(AppError::Forbidden(
                "the path leads outside the folder".into(),
            ));
        }
        let result = if meta.is_dir() {
            std::fs::remove_dir_all(&target.path)
        } else {
            std::fs::remove_file(&target.path)
        };
        result.map_err(|e| io_error(&e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_relative_paths_pass() {
        assert_eq!(
            relative_components("src/./main.rs").ok(),
            Some(vec!["src".to_owned(), "main.rs".to_owned()])
        );
        assert_eq!(relative_components("").ok(), Some(Vec::new()));
        assert!(relative_components("../etc/passwd").is_err());
        assert!(relative_components("a/../../b").is_err());
        assert!(relative_components("/etc/passwd").is_err());
        assert!(relative_components("a\0b").is_err());
        assert!(relative_components(".git/config").is_err());
        assert!(relative_components("sub/.git/hooks").is_err());
    }
}
