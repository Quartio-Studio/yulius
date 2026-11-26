//! The one credential wrapper every crate passes secrets in.

use std::fmt;
use std::ops::Deref;

use zeroize::Zeroize;

/// A credential: a token, key or credential file.
///
/// `Debug` prints `Secret(<redacted>)` and there is no `Display` or `Serialize`, so the value
/// cannot reach output by accident. Read it with [`Secret::expose`] only where it leaves the
/// process. The value is wiped from memory when it is dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret<T: Zeroize = String>(T);

impl Secret {
    /// Wraps a credential text.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl Secret<Vec<u8>> {
    /// Wraps a credential file's bytes.
    #[must_use]
    pub fn bytes(value: Vec<u8>) -> Self {
        Self(value)
    }
}

impl<T: Zeroize + Deref> Secret<T> {
    /// The credential value.
    #[must_use]
    pub fn expose(&self) -> &T::Target {
        &self.0
    }
}

impl<T: Zeroize> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

impl<T: Zeroize> Zeroize for Secret<T> {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl<T: Zeroize> Drop for Secret<T> {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::Secret;

    #[test]
    fn debug_is_redacted() {
        let text = Secret::new("sk-ant-oat01-SYNTHETICSYNTHETICSYNTHETIC");
        let bytes = Secret::bytes(b"ghu_SuperSecretValue".to_vec());
        assert_eq!(format!("{text:?}"), "Secret(<redacted>)");
        assert_eq!(format!("{bytes:?}"), "Secret(<redacted>)");
        assert_eq!(text.expose(), "sk-ant-oat01-SYNTHETICSYNTHETICSYNTHETIC");
        assert_eq!(bytes.expose(), b"ghu_SuperSecretValue");
    }
}
