//! Sandbox files for the built-in editor (SPEC-001 EDT-001..004).
//!
//! `root` selects the folder: absent or `sandbox` is the sandbox repository, `task:<task id>`
//! a task worktree of that sandbox. `path` is relative to the root (`""` is the root itself).

use serde::{Deserialize, Serialize};

/// One directory entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// File name.
    pub name: String,
    /// Path relative to the root.
    pub path: String,
    /// `file`, `dir` or `symlink`.
    pub kind: String,
    /// Size in bytes (files).
    pub size: u64,
}

/// `GET /api/sandboxes/{sid}/files`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirListing {
    /// Listed path.
    pub path: String,
    /// Entries: directories first, then files, each alphabetical.
    pub entries: Vec<FileEntry>,
    /// More entries existed than the listing limit.
    pub truncated: bool,
}

/// `GET /api/sandboxes/{sid}/files/content`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileContent {
    /// Path relative to the root.
    pub path: String,
    /// UTF-8 text; `None` for binary or too large files.
    pub content: Option<String>,
    /// The file is not UTF-8 text.
    pub binary: bool,
    /// The file exceeds the editor's size limit.
    pub too_large: bool,
    /// Size in bytes.
    pub size: u64,
    /// SHA-256 hex of the bytes on disk (the base for a later write).
    pub sha256: String,
}

/// `PUT /api/sandboxes/{sid}/files/content`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteFile {
    /// Root (see the module docs).
    #[serde(default)]
    pub root: Option<String>,
    /// Path relative to the root.
    pub path: String,
    /// New UTF-8 content.
    pub content: String,
    /// SHA-256 of the content the edit started from; a different file on disk is a 409.
    /// `None` creates a new file (409 when it exists).
    #[serde(default)]
    pub base_sha256: Option<String>,
}

/// Result of a write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileWritten {
    /// Path relative to the root.
    pub path: String,
    /// SHA-256 of the written bytes.
    pub sha256: String,
    /// Size in bytes.
    pub size: u64,
}

/// `POST /api/sandboxes/{sid}/files`: creates an empty file or a directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateEntry {
    /// Root (see the module docs).
    #[serde(default)]
    pub root: Option<String>,
    /// Path relative to the root.
    pub path: String,
    /// `file` or `dir`.
    pub kind: String,
}

/// Result of an upload (SPEC-001 EDT-006, UI-048): `PUT /api/sandboxes/{sid}/files/upload`
/// (into the repository or a task worktree) and the attachment endpoints
/// (`POST /api/tasks/{tid}/attachments`, `POST /api/sandboxes/{sid}/shells/{id}/attachments`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadedFile {
    /// Editor uploads: the path relative to the root. Attachments: the absolute path the agent
    /// or the shell opens (the same inside containment).
    pub path: String,
    /// The stored file name (attachments may be renamed: safe characters, no clash).
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// SHA-256 hex of the stored bytes (the base for a later editor save).
    pub sha256: String,
    /// An existing file was replaced (editor uploads with `overwrite=true`).
    pub replaced: bool,
}
