//! Account login flows (SPEC-001 ROT-002, SPEC-002 §7.2 "Login", ADR-05).
//!
//! Claude: `claude setup-token` runs in a server-owned PTY; the long-lived OAuth token is
//! captured from its output with [`extract_claude_oauth_token`] while [`TokenMasker`] hides it
//! from viewers. Run the PTY wide (for example 500 columns) so the TUI does not wrap the token
//! across lines; a wrapped token is neither captured nor fully masked.

/// Prefix of Claude long-lived OAuth tokens.
pub const CLAUDE_TOKEN_PREFIX: &str = "sk-ant-oat01-";
/// Minimum number of token characters after the prefix.
pub const CLAUDE_TOKEN_MIN_BODY: usize = 20;
/// Text that replaces a token in masked output.
pub const MASK: &str = "sk-ant-oat01-[REDACTED]";

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// argv of the Claude token login (`claude setup-token`).
#[must_use]
pub fn claude_setup_token_argv() -> Vec<String> {
    vec!["claude".into(), "setup-token".into()]
}

/// The last Claude OAuth token in `output`: `sk-ant-oat01-` followed by at least 20 of
/// `[A-Za-z0-9_-]` (taken greedily).
#[must_use]
pub fn extract_claude_oauth_token(output: &str) -> Option<String> {
    let bytes = output.as_bytes();
    let mut found = None;
    let mut from = 0;
    while let Some(relative) = output[from..].find(CLAUDE_TOKEN_PREFIX) {
        let start = from + relative;
        let body_start = start + CLAUDE_TOKEN_PREFIX.len();
        let body_len = bytes[body_start..]
            .iter()
            .take_while(|&&b| is_token_byte(b))
            .count();
        if body_len >= CLAUDE_TOKEN_MIN_BODY {
            found = Some(output[start..body_start + body_len].to_owned());
        }
        from = body_start + body_len;
    }
    found
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MaskState {
    /// Buffer holds a (possibly empty) proper prefix of the token prefix.
    Scanning,
    /// Buffer holds the full prefix plus fewer than the minimum body characters.
    Candidate,
    /// A token was replaced; further token characters are swallowed.
    Swallowing,
}

/// Streaming filter that replaces Claude OAuth tokens in a byte stream with [`MASK`], also when
/// a token is split across chunks. At most `prefix + 20` bytes are held back.
#[derive(Debug, Clone)]
pub struct TokenMasker {
    state: MaskState,
    held: Vec<u8>,
}

impl Default for TokenMasker {
    fn default() -> Self {
        Self::new()
    }
}

impl TokenMasker {
    /// A masker with no pending bytes.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: MaskState::Scanning,
            held: Vec::new(),
        }
    }

    /// Filters the next chunk and returns the bytes that are safe to show now.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        let prefix = CLAUDE_TOKEN_PREFIX.as_bytes();
        let mut out = Vec::with_capacity(chunk.len());
        for &b in chunk {
            match self.state {
                MaskState::Swallowing => {
                    if !is_token_byte(b) {
                        self.state = MaskState::Scanning;
                        self.scan(b, prefix, &mut out);
                    }
                }
                MaskState::Candidate => {
                    if is_token_byte(b) {
                        self.held.push(b);
                        if self.held.len() >= prefix.len() + CLAUDE_TOKEN_MIN_BODY {
                            self.held.clear();
                            out.extend_from_slice(MASK.as_bytes());
                            self.state = MaskState::Swallowing;
                        }
                    } else {
                        out.append(&mut self.held);
                        self.state = MaskState::Scanning;
                        self.scan(b, prefix, &mut out);
                    }
                }
                MaskState::Scanning => self.scan(b, prefix, &mut out),
            }
        }
        out
    }

    /// Ends the stream and returns any held bytes (an incomplete prefix or a too-short
    /// candidate is not a token).
    pub fn finish(&mut self) -> Vec<u8> {
        self.state = MaskState::Scanning;
        std::mem::take(&mut self.held)
    }

    fn scan(&mut self, b: u8, prefix: &[u8], out: &mut Vec<u8>) {
        self.held.push(b);
        // Keep the longest suffix of `held` that is a prefix of the token prefix.
        while !prefix.starts_with(&self.held) {
            let first = self.held.remove(0);
            out.push(first);
        }
        if self.held.len() == prefix.len() {
            self.state = MaskState::Candidate;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_falls_back_on_partial_prefix() {
        let mut masker = TokenMasker::new();
        let mut out = masker.push(b"sk-ant-sk-ant-oat01-");
        out.extend(masker.push(&[b'A'; 20]));
        out.extend(masker.push(b" done"));
        out.extend(masker.finish());
        assert_eq!(String::from_utf8_lossy(&out), format!("sk-ant-{MASK} done"));
    }
}
