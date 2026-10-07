//! Redacting wrapper for credentials (SEC-002).

use std::fmt;

/// A credential string (token, client secret, private key, webhook secret).
///
/// `Debug` and `Display` print `[redacted]`; the value is reachable only through
/// [`Secret::expose`]. It deliberately implements neither `Serialize` nor `Deserialize`, so
/// it cannot be written to JSON or logs by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// Wraps a credential.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The raw credential, for transmission or encrypted storage only.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

#[cfg(test)]
mod tests {
    use super::Secret;

    #[test]
    fn never_formats_the_value() {
        let secret = Secret::new("ghu_SuperSecretValue");
        assert_eq!(format!("{secret}"), "[redacted]");
        assert!(!format!("{secret:?}").contains("Super"));
        assert_eq!(secret.expose(), "ghu_SuperSecretValue");
    }
}
