//! What the agent CLI installers (`claude_cli`, `pinned_cli`) share: a release tarball streamed
//! to a file under a bound with its sha512, permission bits, and the binary's ELF header check.

use std::path::Path;
use std::time::Duration;

use sha2::{Digest, Sha512};
use tokio::io::AsyncWriteExt as _;

use crate::wiring::claude_cli::UpdateError;

/// Deadline of a tarball's download.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Why a tarball's download failed; each installer words it.
pub(crate) enum DownloadError {
    /// No answer.
    Unreachable,
    /// An unsuccessful HTTP status.
    Status(u16),
    /// Longer than allowed, by its announced length or its bytes.
    TooLarge,
    /// The body stopped.
    Stopped,
    /// The file could not be written.
    Write(std::io::Error),
}

/// Streams the tarball at `url` to `to`, at most `max` bytes; its sha512.
pub(crate) async fn download(
    http: &reqwest::Client,
    url: &str,
    max: u64,
    to: &Path,
) -> Result<Vec<u8>, DownloadError> {
    let mut response = http
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|_| DownloadError::Unreachable)?;
    if !response.status().is_success() {
        return Err(DownloadError::Status(response.status().as_u16()));
    }
    if response.content_length().is_some_and(|n| n > max) {
        return Err(DownloadError::TooLarge);
    }
    let mut file = tokio::fs::File::create(to)
        .await
        .map_err(DownloadError::Write)?;
    let mut hasher = Sha512::new();
    let mut size = 0u64;
    while let Some(chunk) = response.chunk().await.map_err(|_| DownloadError::Stopped)? {
        size += chunk.len() as u64;
        if size > max {
            return Err(DownloadError::TooLarge);
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(DownloadError::Write)?;
    }
    file.flush().await.map_err(DownloadError::Write)?;
    Ok(hasher.finalize().to_vec())
}

/// Sets `path`'s permission bits.
pub(crate) async fn set_mode(path: &Path, mode: u32) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt as _;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .await
        .map_err(|e| UpdateError::new(format!("cannot set the mode of {}: {e}", path.display())))
}

/// Whether `header`, a binary's first 20 bytes, is a 64-bit little-endian ELF for `machine`.
pub(crate) fn elf64_for(header: &[u8], machine: u16) -> bool {
    header.len() >= 20
        && header[..4] == *b"\x7fELF"
        && header[4] == 2
        && header[5] == 1
        && u16::from_le_bytes([header[18], header[19]]) == machine
}
