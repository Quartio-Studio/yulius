//! Account login flows (SPEC-001 ROT-002, SPEC-002 §7.2 "Login", ADR-05).
//!
//! Claude: `claude setup-token` runs in a server-owned PTY; the long-lived OAuth token is
//! captured from its output with [`extract_claude_oauth_token`] while [`TokenMasker`] hides it
//! from viewers. Run the PTY wide (for example 500 columns) so the TUI does not wrap the token
//! across lines; a wrapped token is neither captured nor fully masked.
//!
//! Codex (SPEC-451 CDX-005): `codex login --device-auth` runs with a fresh temporary `CODEX_HOME`
//! whose `config.toml` keeps credentials in `auth.json` (never the OS keyring); that file is the
//! account's credential, validated by [`validate_codex_auth`]. An API key becomes an `auth.json`
//! through [`codex_api_key_auth`]. After each run the copy with the newer `last_refresh` wins
//! ([`newer_codex_auth`]), so a token Codex refreshed during a run is kept.

use std::cmp::Ordering;

use serde_json::{Value, json};

use yulius_domain::time::Timestamp;

use crate::time::parse_rfc3339;

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

/// argv of the Codex device-code login (`codex login --device-auth`); run it with `CODEX_HOME` set
/// to a fresh private directory holding [`CODEX_FILE_STORE_CONFIG`] as its `config.toml`.
#[must_use]
pub fn codex_device_login_argv() -> Vec<String> {
    vec!["codex".into(), "login".into(), "--device-auth".into()]
}

/// The `config.toml` of a Codex home Yulius prepares: credentials live in `auth.json`, never the
/// OS keyring, so they can be moved into the vault and written back after a run.
pub const CODEX_FILE_STORE_CONFIG: &str = "cli_auth_credentials_store = \"file\"\n";

/// Largest Codex `auth.json` accepted.
pub const CODEX_AUTH_MAX_BYTES: usize = 1024 * 1024;

/// Why a Codex credential is not usable (no secret in the text).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CodexAuthError {
    /// Empty or larger than [`CODEX_AUTH_MAX_BYTES`].
    #[error("the Codex credentials are empty or too large")]
    Size,
    /// Not a JSON object.
    #[error("the Codex credentials are not valid JSON")]
    NotJson,
    /// Neither an API key nor ChatGPT tokens.
    #[error("the Codex credentials hold neither an API key nor sign-in tokens")]
    NoCredential,
    /// An imported file that is not a ChatGPT sign-in (`auth_mode` is not `chatgpt`).
    #[error("the file is not a ChatGPT sign-in of Codex (its auth_mode is not \"chatgpt\")")]
    NotChatGpt,
    /// An imported ChatGPT sign-in without all of its tokens or its account.
    #[error("the ChatGPT sign-in lacks its id_token, access_token, refresh_token or account_id")]
    MissingTokens,
}

/// Checks a Codex `auth.json`: a bounded JSON object holding a non-empty `OPENAI_API_KEY` or
/// `tokens` with a non-empty `access_token` or `refresh_token`.
///
/// # Errors
/// [`CodexAuthError`] saying what is wrong.
pub fn validate_codex_auth(auth_json: &[u8]) -> Result<(), CodexAuthError> {
    if auth_json.is_empty() || auth_json.len() > CODEX_AUTH_MAX_BYTES {
        return Err(CodexAuthError::Size);
    }
    let Ok(Value::Object(auth)) = serde_json::from_slice::<Value>(auth_json) else {
        return Err(CodexAuthError::NotJson);
    };
    let filled = |v: Option<&Value>| v.and_then(Value::as_str).is_some_and(|s| !s.is_empty());
    let key = filled(auth.get("OPENAI_API_KEY"));
    let tokens = auth
        .get("tokens")
        .and_then(Value::as_object)
        .is_some_and(|t| filled(t.get("access_token")) || filled(t.get("refresh_token")));
    if key || tokens {
        Ok(())
    } else {
        Err(CodexAuthError::NoCredential)
    }
}

/// Checks an `auth.json` imported from a `codex login` made elsewhere (SPEC-451 CDX-011): a
/// [`validate_codex_auth`] credential with `auth_mode` `chatgpt` and non-empty `tokens.id_token`,
/// `access_token`, `refresh_token` and `account_id`, which [`codex_auth_replaces`] needs to take
/// its refreshes back.
///
/// # Errors
/// [`CodexAuthError`] saying what is wrong, never quoting the file.
pub fn validate_codex_import(auth_json: &[u8]) -> Result<(), CodexAuthError> {
    validate_codex_auth(auth_json)?;
    let Ok(Value::Object(auth)) = serde_json::from_slice::<Value>(auth_json) else {
        return Err(CodexAuthError::NotJson);
    };
    if auth.get("auth_mode").and_then(Value::as_str) != Some("chatgpt") {
        return Err(CodexAuthError::NotChatGpt);
    }
    let tokens = auth.get("tokens").and_then(Value::as_object);
    let filled = |name: &str| {
        tokens
            .and_then(|t| t.get(name))
            .and_then(Value::as_str)
            .is_some_and(|s| !s.trim().is_empty())
    };
    if ["id_token", "access_token", "refresh_token", "account_id"]
        .into_iter()
        .all(filled)
    {
        Ok(())
    } else {
        Err(CodexAuthError::MissingTokens)
    }
}

/// Whether `key` looks like an OpenAI API key: `sk-` and 20 to 512 of `[A-Za-z0-9_-]`.
#[must_use]
pub fn valid_openai_api_key(key: &str) -> bool {
    key.strip_prefix("sk-")
        .is_some_and(|body| (20..=512).contains(&body.len()) && body.bytes().all(is_token_byte))
}

/// The `auth.json` of an API-key account, as `codex login --with-api-key` writes it.
#[must_use]
pub fn codex_api_key_auth(key: &str) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(&json!({
        "auth_mode": "apikey",
        "OPENAI_API_KEY": key,
    }))
    .unwrap_or_default();
    bytes.push(b'\n');
    bytes
}

/// `last_refresh` of a Codex `auth.json`, if present.
#[must_use]
pub fn codex_auth_last_refresh(auth_json: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(auth_json).ok()?;
    value
        .get("last_refresh")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Compares two Codex `auth.json` contents by `last_refresh`: `Greater` when `a` is newer.
/// A parseable time beats an unparseable or missing one; two missing times are `Equal`;
/// unparseable texts compare lexically.
#[must_use]
pub fn newer_codex_auth(a: &[u8], b: &[u8]) -> Ordering {
    let (ra, rb) = (codex_auth_last_refresh(a), codex_auth_last_refresh(b));
    let parsed = |r: &Option<String>| r.as_deref().and_then(parse_rfc3339);
    match (parsed(&ra), parsed(&rb)) {
        (Some(x), Some(y)) => x.cmp(&y),
        (Some(_), None) => Ordering::Greater,
        (None, Some(_)) => Ordering::Less,
        (None, None) => ra.cmp(&rb),
    }
}

/// How far ahead of the server's clock a refreshed `auth.json` may be dated.
pub const CODEX_REFRESH_SKEW_MS: i64 = 5 * 60 * 1000;

/// Whether the `auth.json` a Codex run left (`fresh`) replaces the account's stored one: Codex
/// refreshed the same ChatGPT sign-in (same `tokens.account_id`, a usable credential) at a
/// `last_refresh` newer than the stored one and at most [`CODEX_REFRESH_SKEW_MS`] after `now`,
/// of the same signed-in user when the stored `id_token` names one.
/// The file lies in the agent's sandbox: one it wrote itself (another account, an API key, a date
/// ahead so that real refreshes never win) is not taken. An API-key account is never replaced.
#[must_use]
pub fn codex_auth_replaces(fresh: &[u8], stored: &[u8], now: Timestamp) -> bool {
    let account = |auth: &[u8]| -> Option<String> {
        let value: Value = serde_json::from_slice(auth).ok()?;
        value
            .get("tokens")?
            .get("account_id")?
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    };
    let Some(refreshed) = codex_auth_last_refresh(fresh)
        .as_deref()
        .and_then(parse_rfc3339)
    else {
        return false;
    };
    // In a Team workspace every member shares `account_id`: the signed-in user must match too.
    let user_matches = match signed_in_user(stored) {
        Some(user) => signed_in_user(fresh).as_deref() == Some(user.as_str()),
        None => true,
    };
    validate_codex_auth(fresh).is_ok()
        && account(stored).is_some()
        && account(fresh) == account(stored)
        && user_matches
        && refreshed.0 <= now.0.saturating_add(CODEX_REFRESH_SKEW_MS)
        && newer_codex_auth(fresh, stored) == Ordering::Greater
}

/// The signed-in user of a ChatGPT `auth.json`: the `sub` claim of its `id_token` (read, not
/// verified: it only tells two sign-ins apart).
fn signed_in_user(auth_json: &[u8]) -> Option<String> {
    use base64::Engine as _;
    let value: Value = serde_json::from_slice(auth_json).ok()?;
    let token = value.get("tokens")?.get("id_token")?.as_str()?;
    let payload = token.split('.').nth(1)?;
    let claims: Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload.trim_end_matches('='))
            .ok()?,
    )
    .ok()?;
    claims
        .get("sub")
        .and_then(Value::as_str)
        .filter(|sub| !sub.is_empty())
        .map(str::to_owned)
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
