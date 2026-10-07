//! Application errors mapped to API error codes.

/// A use-case failure. Messages never contain secrets.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppError {
    /// No valid session or token.
    #[error("authentication required")]
    Unauthenticated,
    /// Authenticated but not allowed.
    #[error("{0}")]
    Forbidden(String),
    /// Missing or invisible object.
    #[error("not found")]
    NotFound,
    /// State changed concurrently or the request conflicts with current state.
    #[error("{0}")]
    Conflict(String),
    /// Invalid input.
    #[error("{0}")]
    Invalid(String),
    /// A capacity bound was reached.
    #[error("{0}")]
    Capacity(String),
    /// GitHub, a provider or another dependency failed.
    #[error("{0}")]
    Upstream(String),
    /// Unexpected internal failure (details are logged, not returned).
    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    /// Stable API error code.
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden(_) => "forbidden",
            Self::NotFound => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Invalid(_) => "invalid",
            Self::Capacity(_) => "capacity",
            Self::Upstream(_) => "upstream",
            Self::Internal(_) => "internal",
        }
    }

    /// Message safe to return to clients.
    #[must_use]
    pub fn public_message(&self) -> String {
        match self {
            Self::Internal(_) => "internal error".to_owned(),
            other => other.to_string(),
        }
    }

    /// Shorthand for an internal error from any displayable cause.
    pub fn internal(cause: impl std::fmt::Display) -> Self {
        Self::Internal(cause.to_string())
    }
}

/// Result alias for application operations.
pub type AppResult<T> = Result<T, AppError>;
