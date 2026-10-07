//! Secret values that never print (SPEC-002 §8, SEC-002).

use std::fmt;

/// A secret value whose `Debug` output is redacted. Read it only with [`Secret::expose`] at
/// the point where it leaves the process (environment variable or 0600 file).
#[derive(Clone, PartialEq, Eq)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    /// Wraps a secret value.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// The secret value.
    #[must_use]
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_redacted() {
        let secret = Secret::new("sk-ant-oat01-SYNTHETICSYNTHETICSYNTHETIC".to_owned());
        let printed = format!("{secret:?}");
        assert!(!printed.contains("SYNTHETIC"));
        assert!(secret.expose().ends_with("SYNTHETIC"));
    }
}
