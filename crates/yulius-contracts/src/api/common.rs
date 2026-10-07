//! Shared API shapes: errors and small references.

use serde::{Deserialize, Serialize};

/// Error body: `{"error": {"code", "message"}}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// The error.
    pub error: ErrorDetail,
}

/// Error code and human-readable message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDetail {
    /// Stable snake_case code (`unauthenticated`, `forbidden`, `not_found`, `conflict`,
    /// `invalid`, `capacity`, `upstream`, `unavailable`, `internal`).
    pub code: String,
    /// Human-readable explanation without secrets.
    pub message: String,
}

/// A reference to a person by GitHub login.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserRef {
    /// GitHub login (or `superadmin`).
    pub login: String,
}

/// Generic acknowledgement with an optional identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdResponse {
    /// Identifier of the created or affected object.
    pub id: String,
}
