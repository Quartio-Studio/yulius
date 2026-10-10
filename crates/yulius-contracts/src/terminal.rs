//! Terminal WebSocket frames between browser and server (docs/api.md, SPEC-002 §7.7).

use serde::{Deserialize, Serialize};

/// Binary frame kind: live output.
pub const KIND_OUTPUT: u8 = 1;
/// Binary frame kind: full-screen snapshot (clear, then write).
pub const KIND_SNAPSHOT: u8 = 2;
/// Close code sent to a viewer that fell behind.
pub const CLOSE_LAGGING: u16 = 4008;

/// Encodes a binary terminal frame: `kind`, big-endian `seq`, bytes.
#[must_use]
pub fn encode_binary(kind: u8, seq: u64, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + bytes.len());
    out.push(kind);
    out.extend_from_slice(&seq.to_be_bytes());
    out.extend_from_slice(bytes);
    out
}

/// Decodes a binary terminal frame into `(kind, seq, bytes)`.
#[must_use]
pub fn decode_binary(frame: &[u8]) -> Option<(u8, u64, &[u8])> {
    let (&kind, rest) = frame.split_first()?;
    let seq_bytes: [u8; 8] = rest.get(..8)?.try_into().ok()?;
    Some((kind, u64::from_be_bytes(seq_bytes), rest.get(8..)?))
}

/// Text frames from the browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalClientMsg {
    /// First frame after connecting.
    Hello {
        /// Last output sequence the client rendered (for diagnostics; a snapshot is sent anyway).
        #[serde(default)]
        last_seq: Option<u64>,
        /// Client terminal columns.
        cols: u16,
        /// Client terminal rows.
        rows: u16,
    },
    /// Keyboard input.
    Input {
        /// Text or escape sequences.
        data: String,
        /// Client input sequence for duplicate suppression after reconnect.
        #[serde(default)]
        seq: Option<u64>,
        /// A wheel turn forwarded to a full-screen app: reaches the app without taking control
        /// of the terminal (or its size) (#78).
        #[serde(default)]
        scroll: bool,
    },
    /// The size this viewer's window fits now; the terminal takes it while this viewer
    /// controls the size.
    Resize {
        /// Columns.
        cols: u16,
        /// Rows.
        rows: u16,
    },
    /// The viewer clicked into the terminal: it controls the size now (as when it types).
    TakeControl,
    /// The viewer is active (page visible and focused, terminal used recently): it takes the
    /// size from an idle controller.
    Active,
    /// The viewer is idle (hidden, unfocused or unused for a while): when it controls the size,
    /// the most recently active viewer takes over.
    Idle,
}

/// Text frames to the browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalServerMsg {
    /// Session facts; sent on connect and on change.
    State {
        /// Attempt sequence currently attached.
        attempt_seq: Option<u32>,
        /// Harness of the attempt.
        harness: Option<String>,
        /// PTY columns.
        cols: u16,
        /// PTY rows.
        rows: u16,
        /// Whether this viewer may type.
        can_write: bool,
        /// Whether this viewer controls the size.
        controller: bool,
        /// Task status wire name.
        status: String,
    },
    /// A short informational line (e.g. account switch).
    Notice {
        /// Text.
        text: String,
    },
    /// The run ended.
    Exit {
        /// Task status after the exit.
        status: String,
        /// Exit code when known.
        code: Option<i32>,
    },
    /// A protocol or permission error.
    Error {
        /// Code.
        code: String,
        /// Message.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_round_trip_and_truncation() {
        let frame = encode_binary(KIND_OUTPUT, 42, b"hi");
        assert_eq!(decode_binary(&frame), Some((KIND_OUTPUT, 42, &b"hi"[..])));
        assert_eq!(decode_binary(&frame[..5]), None);
        assert_eq!(decode_binary(&[]), None);
    }

    #[test]
    fn client_json_shapes() -> Result<(), serde_json::Error> {
        let hello: TerminalClientMsg =
            serde_json::from_str(r#"{"type":"hello","last_seq":null,"cols":80,"rows":24}"#)?;
        assert_eq!(
            hello,
            TerminalClientMsg::Hello {
                last_seq: None,
                cols: 80,
                rows: 24
            }
        );
        let input: TerminalClientMsg = serde_json::from_str(r#"{"type":"input","data":"ls\r"}"#)?;
        assert_eq!(
            input,
            TerminalClientMsg::Input {
                data: "ls\r".into(),
                seq: None,
                scroll: false
            }
        );
        let tc: TerminalClientMsg = serde_json::from_str(r#"{"type":"take_control"}"#)?;
        assert_eq!(tc, TerminalClientMsg::TakeControl);
        let active: TerminalClientMsg = serde_json::from_str(r#"{"type":"active"}"#)?;
        assert_eq!(active, TerminalClientMsg::Active);
        let idle: TerminalClientMsg = serde_json::from_str(r#"{"type":"idle"}"#)?;
        assert_eq!(idle, TerminalClientMsg::Idle);
        Ok(())
    }
}
