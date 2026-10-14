//! Auto-saved notes (SPEC-041): one notepad per sandbox, one for the team and one per person
//! (SPEC-103), and who has a shared one open (SPEC-112).

use serde::{Deserialize, Serialize};

/// A notepad: `GET /api/notes` (team), `GET /api/sandboxes/{sid}/notes` (sandbox) and
/// `GET /api/me/notes` (the caller's own); `PUT` returns the stored note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Note {
    /// `team`, `sandbox` or `me`.
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

/// A selection in a notepad: UTF-16 offsets into its text, `anchor` where it started and `head`
/// where the caret is (equal for a caret alone).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoteSelection {
    /// Where the selection started.
    pub anchor: u32,
    /// Where it ends: the caret.
    pub head: u32,
}

/// `PUT /api/notes/presence` and `PUT /api/sandboxes/{sid}/notes/presence` body (SPEC-112): the
/// caller's selection in the text at `revision`, or `null` when they left the notepad. Relayed
/// to the notepad's readers as `note.presence` and stored nowhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotePresence {
    /// The revision whose text the offsets are in.
    pub revision: u64,
    /// The selection, or `null`: gone from the notepad.
    pub selection: Option<NoteSelection>,
    /// The notepad was just shown: the others answer with their own presence.
    #[serde(default)]
    pub hello: bool,
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

    #[test]
    fn presence_refuses_unknown_fields_and_says_hello_only_when_asked() {
        let left = serde_json::from_str::<NotePresence>(r#"{"revision":2,"selection":null}"#);
        assert!(left.is_ok_and(|left| left.selection.is_none() && !left.hello));
        assert!(
            serde_json::from_str::<NotePresence>(r#"{"revision":2,"selection":null,"x":1}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<NotePresence>(
                r#"{"revision":2,"selection":{"anchor":1,"head":2,"name":"x"}}"#
            )
            .is_err()
        );
    }
}
