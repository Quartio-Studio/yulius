//! The openvscode-server connection token: random, private on disk, redacted in logs.

use std::fmt;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use tokio::io::AsyncReadExt as _;

/// Cookie name openvscode-server accepts the connection token from.
pub const TOKEN_COOKIE: &str = "vscode-tkn";

/// A 256-bit random connection token (64 hex characters). `Debug` and `Display` are redacted;
/// use [`ConnectionToken::expose_secret`] only where the token must be sent.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionToken(String);

impl ConnectionToken {
    /// Generates a token from the operating system's random source.
    ///
    /// # Errors
    /// When `/dev/urandom` cannot be read.
    pub async fn generate() -> std::io::Result<Self> {
        let mut bytes = [0_u8; 32];
        let mut source = tokio::fs::File::open("/dev/urandom").await?;
        source.read_exact(&mut bytes).await?;
        Ok(Self(bytes.iter().map(|b| format!("{b:02x}")).collect()))
    }

    /// The token text (send only to the editor, e.g. in the proxy's cookie header).
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }

    /// `vscode-tkn=<token>`: the `Cookie` value a reverse proxy adds to every request and
    /// WebSocket upgrade it forwards to the editor.
    #[must_use]
    pub fn cookie_value(&self) -> String {
        format!("{TOKEN_COOKIE}={}", self.0)
    }
}

impl fmt::Debug for ConnectionToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ConnectionToken(<redacted>)")
    }
}

impl fmt::Display for ConnectionToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Writes the token to `path` with mode 0600 via a same-directory temporary file and rename,
/// so the file is never readable by others and never half-written.
pub(crate) fn write_token_file(path: &Path, token: &ConnectionToken) -> std::io::Result<()> {
    let temporary = path.with_extension("tmp");
    let _ = std::fs::remove_file(&temporary);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(token.expose_secret().as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, path)?;
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[tokio::test]
    async fn tokens_are_random_hex_and_redacted() -> TestResult {
        let first = ConnectionToken::generate().await?;
        let second = ConnectionToken::generate().await?;
        assert_ne!(first, second);
        assert_eq!(first.expose_secret().len(), 64);
        assert!(first.expose_secret().bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(format!("{first:?}"), "ConnectionToken(<redacted>)");
        assert_eq!(first.to_string(), "<redacted>");
        assert_eq!(
            first.cookie_value(),
            format!("vscode-tkn={}", first.expose_secret())
        );
        Ok(())
    }

    #[tokio::test]
    async fn token_file_is_private_and_replaced_atomically() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("connection-token");
        std::fs::write(&path, "old")?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
        let token = ConnectionToken::generate().await?;
        write_token_file(&path, &token)?;
        assert_eq!(std::fs::read_to_string(&path)?, token.expose_secret());
        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            0o600
        );
        assert!(!path.with_extension("tmp").exists());
        Ok(())
    }
}
