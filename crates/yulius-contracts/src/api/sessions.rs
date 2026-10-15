//! Sessions as read-only documents: task sessions and imported Claude Code sessions of a sandbox
//! (SPEC-001 SES-001..006, docs/api.md "Sessions").

use serde::{Deserialize, Serialize};

use super::common::UserRef;

/// `task`: the Claude Code session of a Yulius task. `imported`: a session someone uploaded
/// from their computer with `yulius onboard`.
pub const SESSION_KINDS: [&str; 2] = ["task", "imported"];

/// A session in lists and search results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDoc {
    /// Session document id (opaque).
    pub id: String,
    /// Sandbox id.
    pub sandbox_id: String,
    /// `task` or `imported`.
    pub kind: String,
    /// Title: the task title, or the session's first prompt.
    pub title: String,
    /// Harness wire name (`claude`).
    pub harness: String,
    /// The harness's own session id.
    pub session_id: String,
    /// Who uploaded it (imported) or created the task (task).
    pub author: Option<UserRef>,
    /// The task, for task sessions.
    pub task: Option<SessionTask>,
    /// The task an imported session was continued in, if any (IMP-004).
    pub continued_in: Option<SessionTask>,
    /// Git branch recorded when the session started.
    pub git_branch: Option<String>,
    /// First recorded time (RFC 3339).
    pub started_at: Option<String>,
    /// Last recorded time (RFC 3339).
    pub ended_at: Option<String>,
    /// When it was uploaded (imported sessions, RFC 3339).
    pub imported_at: Option<String>,
    /// Transcript size in bytes.
    pub bytes: u64,
    /// Secrets the CLI redacted before upload (imported sessions).
    pub redactions: u32,
    /// The caller may continue it as a new task (imported, write access).
    pub can_continue: bool,
    /// The caller may delete it (the uploader or an admin; imported only).
    pub can_delete: bool,
    /// Matching passages (search results only), best first.
    #[serde(default)]
    pub hits: Vec<SessionHit>,
}

/// The task behind a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTask {
    /// Task id.
    pub id: String,
    /// Task number.
    pub number: u32,
    /// Task title.
    pub title: String,
    /// Task status wire name.
    pub status: String,
}

/// One search match inside a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHit {
    /// Transcript line of the match (open the transcript there).
    pub line: u64,
    /// `user`, `assistant`, `tool` or `summary`.
    pub role: String,
    /// The passage; parts with `hit: true` matched the query.
    pub snippet: Vec<SnippetPart>,
}

/// A piece of a snippet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetPart {
    /// Text.
    pub text: String,
    /// Whether this part matched the query.
    pub hit: bool,
}

/// `GET /api/sandboxes/{sid}/sessions` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionList {
    /// Sessions, newest first (search: best match first).
    pub sessions: Vec<SessionDoc>,
    /// Every author of the sandbox's sessions (for the person filter).
    pub authors: Vec<String>,
    /// Sessions matching the filters before the limit.
    pub total: u32,
    /// The search text, when searching.
    pub query: Option<String>,
}

/// `GET /api/sessions/{id}/transcript` response: a bounded page of normalized entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptPage {
    /// The session.
    pub session: SessionDoc,
    /// Entries in transcript order.
    pub entries: Vec<TranscriptEntry>,
    /// The `from` of the next page; `None` at the end of the transcript.
    pub next: Option<u64>,
    /// Lines skipped as unreadable or over the line bound in this page.
    pub skipped_lines: u32,
}

/// One transcript line rendered for reading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptEntry {
    /// Line number in the transcript file (0-based).
    pub line: u64,
    /// `user`, `assistant`, `system` or `summary`.
    pub role: String,
    /// Recorded time (RFC 3339), when present.
    pub at: Option<String>,
    /// A subagent's message (Claude Code sidechain).
    pub sidechain: bool,
    /// Content blocks.
    pub blocks: Vec<TranscriptBlock>,
}

/// A content block. Long text is cut (`truncated`) at the server's bounds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptBlock {
    /// Message text (Markdown).
    Text {
        /// Text.
        text: String,
        /// Cut at the bound.
        truncated: bool,
    },
    /// The model's visible reasoning.
    Thinking {
        /// Text.
        text: String,
        /// Cut at the bound.
        truncated: bool,
    },
    /// A tool call.
    ToolCall {
        /// Tool use id.
        id: String,
        /// Tool name (`Bash`, `Edit`, `mcp__yulius__tasks_list`, …).
        name: String,
        /// One-line summary (command, path, pattern).
        summary: String,
        /// Pretty-printed input (bounded).
        input: String,
        /// File changes of edit tools.
        diff: Option<TranscriptDiff>,
    },
    /// A tool's result.
    ToolResult {
        /// The call it answers.
        tool_use_id: String,
        /// Result text (bounded).
        text: String,
        /// The tool reported an error.
        is_error: bool,
        /// Cut at the bound.
        truncated: bool,
    },
    /// An image (not stored by imports; shown as a placeholder).
    Image {
        /// Media type, when known.
        media_type: Option<String>,
    },
    /// Harness bookkeeping (commands, reminders, interruptions), shown quietly.
    Meta {
        /// Text.
        text: String,
        /// Cut at the bound (additive: absent reads as `false`).
        #[serde(default)]
        truncated: bool,
    },
}

/// A file change made by an edit tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptDiff {
    /// File path as the tool saw it.
    pub path: String,
    /// Replaced passages (a `Write` is one hunk with an empty `old`).
    pub hunks: Vec<DiffHunk>,
    /// A side of a hunk was cut at the bound, or hunks past the most kept were dropped
    /// (additive: absent reads as `false`).
    #[serde(default)]
    pub truncated: bool,
}

/// One replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    /// Text before.
    pub old: String,
    /// Text after.
    pub new: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_are_tagged_by_type() -> Result<(), serde_json::Error> {
        let block = TranscriptBlock::ToolCall {
            id: "toolu_1".into(),
            name: "Edit".into(),
            summary: "src/a.rs".into(),
            input: "{}".into(),
            diff: Some(TranscriptDiff {
                path: "src/a.rs".into(),
                hunks: vec![DiffHunk {
                    old: "a".into(),
                    new: "b".into(),
                }],
                truncated: false,
            }),
        };
        let json = serde_json::to_value(&block)?;
        assert_eq!(json["type"], "tool_call");
        assert_eq!(json["diff"]["hunks"][0]["new"], "b");
        let back: TranscriptBlock = serde_json::from_value(json)?;
        assert_eq!(back, block);
        let text: TranscriptBlock =
            serde_json::from_str(r#"{"type":"text","text":"hi","truncated":false}"#)?;
        assert!(matches!(text, TranscriptBlock::Text { .. }));
        Ok(())
    }
}
