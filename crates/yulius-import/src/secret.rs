//! Redacted credential wrapper (SPEC-002 §8: secrets never logged).

use std::fmt;

/// A bearer token or other credential.
///
/// `Debug` prints `Secret(<redacted>)` and there is no `Display` or `Serialize`, so the value
/// cannot reach output by accident; [`Secret::expose`] is used only where the value is sent.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a credential value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The credential value, for building an `Authorization` header or persisting it.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::Secret;

    #[test]
    fn debug_is_redacted() {
        let secret = Secret::new("yul_top_secret");
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        assert_eq!(secret.expose(), "yul_top_secret");
    }
}
