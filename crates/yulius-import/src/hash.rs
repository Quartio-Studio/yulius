//! Streaming SHA-256 of files.

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};
use yulius_contracts::import::FileRef;

const BUFFER_BYTES: usize = 256 * 1024;

/// Size and SHA-256 of the file at `path`, named `name` in the manifest.
pub(crate) fn file_ref(path: &Path, name: &str) -> io::Result<FileRef> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let size = copy_into_hasher(&mut file, &mut hasher, u64::MAX)?;
    Ok(FileRef {
        name: name.to_owned(),
        size,
        sha256: hex::encode(hasher.finalize()),
    })
}

/// Feeds up to `limit` bytes from `reader` into `hasher`; returns the bytes read.
pub(crate) fn copy_into_hasher<R: Read>(
    reader: &mut R,
    hasher: &mut Sha256,
    limit: u64,
) -> io::Result<u64> {
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    let mut total = 0_u64;
    while total < limit {
        let want =
            usize::try_from((limit - total).min(BUFFER_BYTES as u64)).unwrap_or(BUFFER_BYTES);
        let read = reader.read(&mut buffer[..want])?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
    Ok(total)
}

/// Lowercase hex SHA-256 of `bytes`.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_digests() -> io::Result<()> {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("f");
        std::fs::write(&path, b"abc")?;
        let reference = file_ref(&path, "f")?;
        assert_eq!(reference.size, 3);
        assert_eq!(reference.sha256, sha256_hex(b"abc"));
        let mut hasher = Sha256::new();
        assert_eq!(copy_into_hasher(&mut &b"abcdef"[..], &mut hasher, 3)?, 3);
        assert_eq!(hex::encode(hasher.finalize()), sha256_hex(b"abc"));
        Ok(())
    }
}
