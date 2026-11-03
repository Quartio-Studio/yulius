//! Sandbox files for the built-in editor (SPEC-001 EDT-001..004).
//!
//! `root` selects the folder: absent or `sandbox` is the sandbox repository, `task:<task id>`
//! a task worktree of that sandbox. `path` is relative to the root (`""` is the root itself).
//! Who has a file open in the editor is relayed, not stored (SPEC-127).

use serde::{Deserialize, Serialize};

use crate::api::notes::NoteSelection;

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

/// One folder the editor can open (SPEC-340 EWT-001).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRoot {
    /// `sandbox` or `task:<task id>`, the `root` of the other file requests.
    pub root: String,
    /// The branch checked out there; `None` for a detached or unreadable `HEAD`.
    pub branch: Option<String>,
    /// The task's number, for a task worktree.
    pub task_number: Option<u32>,
    /// The task's title, for a task worktree.
    pub task_title: Option<String>,
    /// The caller may change files there (a task's worktree is its owner's, SPEC-070).
    pub writable: bool,
}

/// `GET /api/sandboxes/{sid}/files/roots`: the sandbox repository, then every task worktree on
/// disk, newest task first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRoots {
    /// The folders.
    pub roots: Vec<FileRoot>,
}

/// `PUT /api/sandboxes/{sid}/files/presence` body (SPEC-127 EDT-008): the caller's selection in
/// the file's text as saved at `sha256`, or `null` when they left the file. Relayed to the
/// sandbox's writers as `file.presence` and stored nowhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilePresence {
    /// `sandbox` (default) or `task:<task id>`, as in the other file requests.
    #[serde(default)]
    pub root: Option<String>,
    /// The file, relative to the root.
    pub path: String,
    /// SHA-256 hex of the saved text the offsets are in (empty: not read yet).
    #[serde(default)]
    pub sha256: String,
    /// The selection in UTF-16 offsets, or `null`: gone from the file.
    pub selection: Option<NoteSelection>,
    /// The file was just shown: the others answer with their own presence.
    #[serde(default)]
    pub hello: bool,
    /// The browser tab (at most 32 letters and digits), so one person's tabs are told apart.
    #[serde(default)]
    pub client: String,
    /// Grows with each update the tab sends, so receivers drop one that arrives after a newer
    /// one (a leave sent as the page goes may overtake it; SPEC-170 TKS-003). 0: not numbered.
    #[serde(default)]
    pub seq: u64,
    /// The live document the offsets are in (SPEC-411 EDT-021); `None`: the text saved at
    /// `sha256`.
    #[serde(default)]
    pub session: Option<String>,
    /// The live document's revision the offsets are in.
    #[serde(default)]
    pub revision: Option<u64>,
}

/// One part of a text operation over UTF-16 units (SPEC-411 EDT-017): a positive count keeps
/// that many units, a negative one deletes that many, a string inserts itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OpPart {
    /// Keep (`n > 0`) or delete (`n < 0`) that many UTF-16 units.
    Count(i64),
    /// Insert this text.
    Insert(String),
}

/// `POST /api/sandboxes/{sid}/files/live`: join a file's shared text (SPEC-411 EDT-017).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveJoin {
    /// `sandbox` (default) or `task:<task id>`.
    #[serde(default)]
    pub root: Option<String>,
    /// The file, relative to the root.
    pub path: String,
    /// The browser tab (as in presence).
    #[serde(default)]
    pub client: String,
    /// The caller's text, to start the document from when none exists (only a caller who may
    /// change the folder; else the disk's text is taken).
    #[serde(default)]
    pub text: Option<String>,
    /// SHA-256 of the file on disk `text` was read from or edited from.
    #[serde(default)]
    pub sha256: Option<String>,
    /// `text` is still the disk's text at `sha256` (nothing unsaved).
    #[serde(default)]
    pub saved: bool,
    /// The line ending the text uses (`\n` or `\r\n`); the disk's text is taken in it.
    #[serde(default)]
    pub eol: Option<String>,
}

/// A file's shared text, as joined or caught up (SPEC-411 EDT-017).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveDoc {
    /// This document's id: a revision is meaningful only within it.
    pub session: String,
    /// The number of steps applied so far.
    pub revision: u64,
    /// The text at `revision` (`None` in a catch-up that answers `steps`).
    pub text: Option<String>,
    /// The steps after the asked revision, oldest first (`None` when the text is answered).
    pub steps: Option<Vec<LiveStep>>,
    /// The line ending of the text.
    pub eol: String,
    /// SHA-256 of the disk's version the text was last saved as or taken from.
    pub sha256: String,
    /// The text at `revision` is what the disk has at `sha256`.
    pub saved: bool,
    /// The disk's newer version while unsaved text is kept (a conflict, EDT-020).
    pub disk_sha256: Option<String>,
}

/// One applied operation (SPEC-411 EDT-017).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveStep {
    /// The revision this step made.
    pub revision: u64,
    /// The person whose tab sent it (`""`: the disk).
    pub user_id: String,
    /// The tab that sent it (`""`: the disk).
    pub client: String,
    /// The operation, at the revision before.
    pub op: Vec<OpPart>,
}

/// `POST /api/sandboxes/{sid}/files/live/edit` (SPEC-411 EDT-017).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveEdit {
    /// `sandbox` (default) or `task:<task id>`.
    #[serde(default)]
    pub root: Option<String>,
    /// The file, relative to the root.
    pub path: String,
    /// The document the operation was made in.
    pub session: String,
    /// The browser tab.
    pub client: String,
    /// The revision the operation was made at.
    pub revision: u64,
    /// The operation.
    pub op: Vec<OpPart>,
}

/// The answer to an edit: the revision its step made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveApplied {
    /// The revision.
    pub revision: u64,
}

/// `POST …/files/live/save` and `POST …/files/live/disk` (SPEC-411 EDT-019, EDT-020).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveFileAction {
    /// `sandbox` (default) or `task:<task id>`.
    #[serde(default)]
    pub root: Option<String>,
    /// The file, relative to the root.
    pub path: String,
    /// The document.
    pub session: String,
    /// Save: write over a disk that moved on (Keep mine). Disk: take the disk's text even over
    /// unsaved text (Take disk version).
    #[serde(default)]
    pub force: bool,
}

/// What a save or a look at the disk left (SPEC-411 EDT-019, EDT-020).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveState {
    /// The document.
    pub session: String,
    /// Its revision.
    pub revision: u64,
    /// SHA-256 of the disk's version the text was last saved as or taken from.
    pub sha256: String,
    /// The text at `revision` is saved.
    pub saved: bool,
    /// The disk's newer version while unsaved text is kept (a conflict).
    pub disk_sha256: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_presence_refuses_unknown_fields_and_needs_a_path() {
        let left = serde_json::from_str::<FilePresence>(r#"{"path":"a.rs","selection":null}"#);
        assert!(left.is_ok_and(|left| left.root.is_none()
            && left.sha256.is_empty()
            && !left.hello
            && left.seq == 0));
        assert!(serde_json::from_str::<FilePresence>(r#"{"selection":null}"#).is_err());
        assert!(
            serde_json::from_str::<FilePresence>(r#"{"path":"a","selection":null,"rev":1}"#)
                .is_err()
        );
    }
}
