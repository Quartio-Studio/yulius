//! The TLS reverse proxy's shared secret (`YULIUS_PROXY_SECRET_FILE`; SPEC-001 ACC-013,
//! docs/operations.md §4): with it, per-client limits key on the client address the proxy passes
//! (`yulius_server::client`). It is read once at start from an owner-only file, like every other
//! Yulius secret file, and never appears in a log, an error or the doctor's output.

use std::path::Path;

use yulius_app::model::Secret;

/// Shortest accepted secret (`openssl rand -hex 32` writes 64 characters).
pub const MIN_SECRET_LEN: usize = 32;
/// Longest accepted secret.
pub const MAX_SECRET_LEN: usize = 4096;

/// Reads the secret from `path`: an owner-only file holding 32–4096 printable ASCII characters
/// without spaces (a valid HTTP header value), surrounding whitespace ignored.
///
/// # Errors
/// A message naming the path and the problem, never the secret.
pub fn read_proxy_secret(path: &Path) -> Result<Secret, String> {
    let text = crate::wiring::jev::read_owner_only_file(path)?;
    let secret = text.trim();
    if (MIN_SECRET_LEN..=MAX_SECRET_LEN).contains(&secret.len())
        && secret.bytes().all(|b| b.is_ascii_graphic())
    {
        Ok(Secret(secret.to_owned()))
    } else {
        Err(format!(
            "{}: the proxy secret must be {MIN_SECRET_LEN}-{MAX_SECRET_LEN} printable characters \
             without spaces (for example from `openssl rand -hex 32`)",
            path.display()
        ))
    }
}

/// The configured secret: `None` when `path` is unset, or when the file is not usable, which is
/// logged without the secret; per-client limits then key on the TCP peer, as without the setting.
#[must_use]
pub fn load(path: Option<&Path>) -> Option<Secret> {
    let path = path?;
    match read_proxy_secret(path) {
        Ok(secret) => {
            tracing::info!(
                "trusted reverse proxy: per-client limits use the client address the proxy passes \
                 with the matching secret"
            );
            Some(secret)
        }
        Err(err) => {
            tracing::warn!(
                "the proxy secret file is not usable: {err}; per-client limits key on the TCP peer"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    const SECRET: &str = "9a8b7c6d5e4f30211203f4e5d6c7b8a99a8b7c6d5e4f30211203f4e5d6c7b8a9";

    fn write(dir: &Path, name: &str, text: &str, mode: u32) -> Result<std::path::PathBuf, String> {
        let path = dir.join(name);
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .map_err(|e| e.to_string())?;
        Ok(path)
    }

    #[test]
    fn reads_an_owner_only_secret_and_refuses_others_without_showing_them()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let good = write(dir.path(), "good", &format!("{SECRET}\n"), 0o600)?;
        assert_eq!(read_proxy_secret(&good)?.expose(), SECRET);
        assert_eq!(load(Some(&good)).as_ref().map(Secret::expose), Some(SECRET));
        assert!(load(None).is_none());
        let readable = write(dir.path(), "readable", SECRET, 0o640)?;
        let short = write(dir.path(), "short", &SECRET[..31], 0o600)?;
        let spaced = write(dir.path(), "spaced", &format!("{SECRET} {SECRET}"), 0o600)?;
        let missing = dir.path().join("missing");
        for (path, expected) in [
            (&readable, "readable by other users (mode 640)"),
            (&short, "32-4096 printable characters"),
            (&spaced, "32-4096 printable characters"),
            (&missing, "cannot read"),
        ] {
            let err = read_proxy_secret(path).err().ok_or("refused")?;
            assert!(err.contains(expected), "{err}");
            assert!(
                !err.contains(&SECRET[..31]),
                "the error shows the secret: {err}"
            );
            assert!(load(Some(path)).is_none());
        }
        Ok(())
    }
}
