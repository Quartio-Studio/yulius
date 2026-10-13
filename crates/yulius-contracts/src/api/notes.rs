//! Auto-saved notes (SPEC-041): one notepad per sandbox and one for the team.

use serde::{Deserialize, Serialize};

/// A notepad: `GET /api/notes` (team) and `GET /api/sandboxes/{sid}/notes` (sandbox); `PUT`
/// returns the stored note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// `team` or `sandbox`.
    pub scope: String,
    /// The text (Markdown, at most 256 KiB of UTF-8).
    pub body: String,
    /// Bumped by every stored edit; 0 for a notepad never written.
    pub revision: u64,
    /// Time of the last edit (RFC 3339); `null` when never written.
    pub updated_at: Option<String>,
    /// Who made the last edit (GitHub login, else display name); `null` when never written.
    pub updated_by: Option<String>,
}

/// `PUT` body: the whole new text and the revision the edit started from. A different stored
/// revision answers `409 conflict` and stores nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveNote {
    /// New text.
    pub body: String,
    /// The revision the edit is based on.
    pub revision: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_note_refuses_unknown_fields() {
        assert!(serde_json::from_str::<SaveNote>(r#"{"body":"x","revision":1}"#).is_ok());
        assert!(
            serde_json::from_str::<SaveNote>(r#"{"body":"x","revision":1,"title":"t"}"#).is_err()
        );
        assert!(serde_json::from_str::<SaveNote>(r#"{"body":"x"}"#).is_err());
    }
}
