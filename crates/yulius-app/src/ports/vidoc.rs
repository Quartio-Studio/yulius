//! Vidoc (SPEC-195 VDK-002): what the instance key's test asks of Vidoc's Messages API. The
//! composition owns the HTTP client; the key never appears in a log or an error.

use async_trait::async_trait;

use crate::model::Secret;

/// What one small test request with a key showed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VidocCheck {
    /// Vidoc answered and lists `vidoc-1`.
    Ok,
    /// The key was refused (HTTP 401 or 403).
    Refused,
    /// Another HTTP error, or Vidoc could not be reached.
    Unreachable,
    /// No answer within the deadline.
    Timeout,
    /// An answer outside the contract (no `vidoc-1` in the model list).
    Invalid,
}

/// Checks Vidoc keys.
#[async_trait]
pub trait VidocConnector: Send + Sync {
    /// One small request with `key` (`GET /v1/models`).
    async fn check(&self, key: &Secret) -> VidocCheck;
}
