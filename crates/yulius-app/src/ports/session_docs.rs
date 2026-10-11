//! Session document persistence (SPEC-001 SES-001..006; SPEC-002 §5 tables `session_docs`,
//! `session_chunks`, `session_fts`).

use async_trait::async_trait;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::time::Timestamp;

use crate::error::AppResult;
use crate::model::ProviderSessionRecord;

/// Where a session document comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionKind {
    /// The Claude Code session of a Yulius task (lineage entry of `provider_sessions`).
    Task,
    /// A session a person uploaded with `yulius onboard`.
    Imported,
}

impl SessionKind {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Imported => "imported",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "task" => Some(Self::Task),
            "imported" => Some(Self::Imported),
            _ => None,
        }
    }
}

/// One session document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDocRecord {
    /// Document id.
    pub id: String,
    /// Sandbox.
    pub sandbox_id: SandboxId,
    /// Origin.
    pub kind: SessionKind,
    /// The task (task sessions).
    pub task_id: Option<TaskId>,
    /// Harness.
    pub harness: Harness,
    /// The harness's session id.
    pub session_id: String,
    /// Title (imported: the first prompt; task sessions show the live task title).
    pub title: String,
    /// Uploader (imported) or task creator (task).
    pub author_id: Option<UserId>,
    /// Branch recorded at the start.
    pub git_branch: Option<String>,
    /// Working directory relative to the checkout.
    pub cwd_relative: Option<String>,
    /// Imported: the revision a continuation's worktree starts from.
    pub base: Option<String>,
    /// Transcript file on the server.
    pub transcript_path: String,
    /// Imported: SHA-256 of the uploaded transcript.
    pub sha256: Option<String>,
    /// Transcript size when stored or last indexed.
    pub bytes: u64,
    /// Secrets redacted by the CLI before upload.
    pub redactions: u32,
    /// First recorded time.
    pub started_at: Option<Timestamp>,
    /// Last recorded time.
    pub ended_at: Option<Timestamp>,
    /// Imported: the task it was continued in.
    pub continued_task_id: Option<TaskId>,
    /// Transcript bytes already indexed.
    pub indexed_bytes: u64,
    /// Transcript lines already indexed.
    pub indexed_lines: u64,
    /// The per-session index bound was reached; the rest is not searchable.
    pub index_full: bool,
    /// Created.
    pub created_at: Timestamp,
    /// Last change (re-import, index progress).
    pub updated_at: Timestamp,
}

/// One indexed passage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionChunk {
    /// Transcript line.
    pub line: u64,
    /// `user`, `assistant`, `tool` or `summary`.
    pub role: String,
    /// Searchable text (redacted, bounded).
    pub text: String,
}

/// Index progress written together with chunks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexAdvance {
    /// Indexed bytes the caller started from (compare-and-set).
    pub expected_bytes: u64,
    /// Indexed bytes after this batch.
    pub bytes: u64,
    /// Indexed lines after this batch.
    pub lines: u64,
    /// The per-session bound was reached.
    pub full: bool,
    /// Current transcript size.
    pub size: u64,
    /// When.
    pub at: Timestamp,
}

/// A full-text match.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionSearchHit {
    /// Document id.
    pub doc_id: String,
    /// Transcript line.
    pub line: u64,
    /// Role of the passage.
    pub role: String,
    /// Snippet with matches between `\u{2}` and `\u{3}`.
    pub snippet: String,
    /// BM25 rank (lower is better).
    pub rank: f64,
}

/// Session document persistence.
#[async_trait]
pub trait SessionDocStore: Send + Sync {
    /// Inserts a document; `Conflict` when the session already has one of the same kind.
    async fn insert_session_doc(&self, doc: &SessionDocRecord) -> AppResult<()>;
    /// Replaces an imported document's content fields (title, author, branch, path, digest,
    /// size, times, redactions, base) and clears its index; `false` when absent.
    async fn replace_session_doc(&self, doc: &SessionDocRecord) -> AppResult<bool>;
    /// Document by id.
    async fn session_doc(&self, id: &str) -> AppResult<Option<SessionDocRecord>>;
    /// The imported document of a session.
    async fn imported_session_doc(
        &self,
        sandbox: &SandboxId,
        harness: Harness,
        session_id: &str,
    ) -> AppResult<Option<SessionDocRecord>>;
    /// Every document of a sandbox.
    async fn session_docs(&self, sandbox: &SandboxId) -> AppResult<Vec<SessionDocRecord>>;
    /// Lineage entries of the sandbox's tasks that have no document yet.
    async fn task_sessions_without_doc(
        &self,
        sandbox: &SandboxId,
    ) -> AppResult<Vec<ProviderSessionRecord>>;
    /// Deletes a document with its index; `false` when absent.
    async fn delete_session_doc(&self, id: &str) -> AppResult<bool>;
    /// Points a document at another transcript file and clears its index.
    async fn set_session_doc_path(&self, id: &str, path: &str) -> AppResult<()>;
    /// Records the task an imported session was continued in.
    async fn set_session_continued(&self, id: &str, task: &TaskId) -> AppResult<()>;
    /// Appends passages and advances the index position, only while it still equals
    /// `advance.expected_bytes`; `false` (nothing written) after a concurrent advance.
    async fn append_session_chunks(
        &self,
        doc_id: &str,
        chunks: &[SessionChunk],
        advance: IndexAdvance,
    ) -> AppResult<bool>;
    /// Drops a document's passages and resets its index position (the transcript shrank).
    async fn reset_session_index(&self, doc_id: &str) -> AppResult<()>;
    /// Full-text matches in a sandbox's documents, best first, at most `limit`.
    async fn search_session_chunks(
        &self,
        sandbox: &SandboxId,
        fts_query: &str,
        limit: u32,
    ) -> AppResult<Vec<SessionSearchHit>>;
    /// Number and total bytes of a sandbox's imported documents.
    async fn imported_session_usage(&self, sandbox: &SandboxId) -> AppResult<(u64, u64)>;
}
