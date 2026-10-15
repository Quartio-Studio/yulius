//! Typed GitHub adapter failures. Messages never include tokens (SEC-002, RUST-007).

use std::time::Duration;

use yulius_domain::redact::{INSTALLATION_TOKEN_PREFIX, stateless_installation_token_len};

/// A GitHub adapter failure.
///
/// Upstream messages are truncated and scrubbed: known request credentials are removed
/// verbatim and GitHub token literals (`ghs_…`, including whole stateless installation tokens
/// `ghs_<APPID>_<JWT>`, `ghu_…`, `github_pat_…`) and JWT-shaped strings are replaced by
/// `[redacted]`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum GitHubError {
    /// 401: missing, invalid or expired credentials.
    #[error("GitHub rejected the credentials (HTTP 401)")]
    Unauthorized,
    /// 403 without rate-limit signals.
    #[error("GitHub refused the request (HTTP 403): {message}")]
    Forbidden {
        /// Scrubbed upstream message.
        message: String,
    },
    /// 404: missing, or invisible to these credentials.
    #[error("GitHub resource not found (HTTP 404)")]
    NotFound,
    /// 409, e.g. the pull request head moved since it was reviewed.
    #[error("GitHub reported a conflict (HTTP 409): {message}")]
    Conflict {
        /// Scrubbed upstream message.
        message: String,
    },
    /// 405 on merge: the pull request is not mergeable (checks, conflicts, already merged).
    #[error("pull request is not mergeable (HTTP 405): {message}")]
    NotMergeable {
        /// Scrubbed upstream message.
        message: String,
    },
    /// 422 on a ref update: the target is not a descendant of the current branch tip.
    #[error("the ref update is not a fast-forward")]
    NotFastForward,
    /// 422 validation failure (name taken, pull request exists, no commits, …).
    #[error("GitHub rejected the request as invalid (HTTP 422): {message}")]
    Invalid {
        /// Scrubbed upstream message.
        message: String,
    },
    /// Primary or secondary rate limit (429, or 403 with rate-limit headers).
    #[error("GitHub rate limit exceeded (retry after {retry_after:?})")]
    RateLimited {
        /// Server-provided wait, when known.
        retry_after: Option<Duration>,
    },
    /// Any other non-success status.
    #[error("GitHub upstream error (HTTP {0})")]
    Upstream(u16),
    /// Connection, TLS or timeout failure before a complete response.
    #[error("transport error talking to GitHub: {0}")]
    Transport(String),
    /// A response body exceeded the adapter's bound.
    #[error("GitHub response exceeded {limit} bytes")]
    ResponseTooLarge {
        /// The bound in bytes.
        limit: usize,
    },
    /// A success response did not have the expected shape.
    #[error("unexpected GitHub response: {0}")]
    Decode(String),
    /// The OAuth endpoint answered with an error code (e.g. `bad_verification_code`).
    #[error("GitHub OAuth error: {0}")]
    OAuth(String),
    /// An argument was rejected before any request was made.
    #[error("invalid {what}: {reason}")]
    InvalidInput {
        /// The kind of argument.
        what: &'static str,
        /// Why it was rejected.
        reason: String,
    },
    /// The operation needs GitHub App credentials, which are not configured.
    #[error("GitHub App credentials are not configured")]
    NotConfigured,
    /// The configured App private key is not a usable RSA key.
    #[error("the GitHub App private key is not a valid RSA key")]
    InvalidKey,
}

const TOKEN_PREFIXES: &[&str] = &["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"];
const MAX_MESSAGE: usize = 300;
const REDACTED: &str = "[redacted]";

/// Truncates `text` and removes credentials from it (`known` values verbatim, token
/// literals and JWT-shaped strings by pattern).
pub(crate) fn scrub(text: &str, known: &[&str]) -> String {
    let mut out = text.to_owned();
    for secret in known {
        if secret.len() >= 4 {
            out = out.replace(secret, REDACTED);
        }
    }
    for prefix in TOKEN_PREFIXES {
        let mut search_from = 0;
        while let Some(found) = out[search_from..].find(prefix) {
            let start = search_from + found;
            let tail = &out[start + prefix.len()..];
            let stateless = if *prefix == INSTALLATION_TOKEN_PREFIX {
                stateless_installation_token_len(tail)
            } else {
                None
            };
            let len = stateless.unwrap_or_else(|| {
                tail.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(tail.len())
            });
            if len == 0 {
                search_from = start + prefix.len();
                continue;
            }
            out.replace_range(start..start + prefix.len() + len, REDACTED);
            search_from = start + REDACTED.len();
        }
    }
    out = scrub_jwts(&out);
    if out.len() > MAX_MESSAGE {
        let mut end = MAX_MESSAGE;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        out.push('…');
    }
    out
}

/// Replaces `eyJ…​.…​.…` (three base64url segments) with `[redacted]`.
fn scrub_jwts(text: &str) -> String {
    let is_b64 = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("eyJ") {
        out.push_str(&rest[..start]);
        let candidate = &rest[start..];
        let end = candidate
            .find(|c: char| !(is_b64(c) || c == '.'))
            .unwrap_or(candidate.len());
        let token = &candidate[..end];
        if token.split('.').filter(|part| !part.is_empty()).count() >= 3 {
            out.push_str(REDACTED);
        } else {
            out.push_str(token);
        }
        rest = &candidate[end..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::scrub;

    #[test]
    fn scrubs_known_and_shaped_credentials() {
        let text = "Bad credentials for ghs_16C7e42F292c6912E7710c838347Ae178B4a and \
                    eyJhbGciOiJSUzI1NiJ9.eyJpc3MiOiIxIn0.c2ln plus custom-token-xyz";
        let clean = scrub(text, &["custom-token-xyz"]);
        assert!(!clean.contains("ghs_16C7"), "{clean}");
        assert!(!clean.contains("eyJhbGci"), "{clean}");
        assert!(!clean.contains("custom-token-xyz"), "{clean}");
        assert!(
            clean.starts_with("Bad credentials for [redacted]"),
            "{clean}"
        );
    }

    /// #424: a stateless installation token (`ghs_<APPID>_<JWT>`) is removed whole; its
    /// payload and signature used to survive as a two-segment remainder.
    #[test]
    fn scrubs_a_whole_stateless_installation_token() {
        let token = format!(
            "ghs_1234567_eyJ{}.eyJ{}.{}",
            "hbGciOiJSUzI1NiJ9".repeat(2),
            "pc3MiOiIxIn0".repeat(4),
            "c2lnbmF0dXJl_-".repeat(4)
        );
        let clean = scrub(&format!("Bad credentials for {token} (installation)"), &[]);
        assert_eq!(clean, "Bad credentials for [redacted] (installation)");
    }

    #[test]
    fn truncates_long_messages() {
        let long = "x".repeat(1000);
        assert!(scrub(&long, &[]).chars().count() <= 301);
    }
}
