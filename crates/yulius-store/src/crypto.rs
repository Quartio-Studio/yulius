//! Crypto adapter: master-key sealing (XChaCha20-Poly1305), Argon2id password hashes, SHA-256
//! and random identifiers (SPEC-002 §8).

use std::fs;
use std::io::Write;
use std::path::Path;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use rand::RngExt as _;
use sha2::{Digest, Sha256};
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::{Sealed, Secret};
use yulius_app::ports::Crypto;

/// Ciphertext format version byte.
const FORMAT_V1: u8 = 1;
const NONCE_LEN: usize = 24;

/// Errors loading the master key.
#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    /// File system failure.
    #[error("master key file: {0}")]
    Io(#[from] std::io::Error),
    /// The key file has the wrong size.
    #[error("master key file must contain exactly 32 bytes")]
    BadLength,
}

/// [`Crypto`] implementation keyed by the instance master key.
pub struct MasterKeyCrypto {
    cipher: XChaCha20Poly1305,
}

impl MasterKeyCrypto {
    /// Loads the 32-byte key from `path`, creating it (mode 0600) when absent.
    ///
    /// # Errors
    /// Fails on I/O errors or a malformed key file.
    pub fn load_or_create(path: &Path) -> Result<Self, KeyError> {
        let key = if path.exists() {
            fs::read(path)?
        } else {
            let mut bytes = [0u8; 32];
            rand::rng().fill(&mut bytes[..]);
            write_private(path, &bytes)?;
            bytes.to_vec()
        };
        Self::from_key(&key)
    }

    /// Builds from raw key bytes (tests).
    ///
    /// # Errors
    /// Fails when the key is not 32 bytes.
    pub fn from_key(key: &[u8]) -> Result<Self, KeyError> {
        if key.len() != 32 {
            return Err(KeyError::BadLength);
        }
        let key = Key::try_from(key).map_err(|_| KeyError::BadLength)?;
        Ok(Self {
            cipher: XChaCha20Poly1305::new(&key),
        })
    }
}

/// Writes a file readable only by the owner (create-new, then fsync).
///
/// # Errors
/// Fails when the file exists or cannot be written.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

impl Crypto for MasterKeyCrypto {
    fn new_id(&self) -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn random_bytes(&self) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        rand::rng().fill(&mut bytes[..]);
        bytes
    }

    fn sha256_hex(&self, data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    fn seal(&self, plaintext: &[u8]) -> AppResult<Sealed> {
        let mut nonce = [0u8; NONCE_LEN];
        rand::rng().fill(&mut nonce[..]);
        let ciphertext = self
            .cipher
            .encrypt(&XNonce::from(nonce), plaintext)
            .map_err(|_| AppError::Internal("seal failed".into()))?;
        let mut out = Vec::with_capacity(1 + NONCE_LEN + ciphertext.len());
        out.push(FORMAT_V1);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ciphertext);
        Ok(Sealed(out))
    }

    fn open(&self, sealed: &Sealed) -> AppResult<Vec<u8>> {
        let bytes = &sealed.0;
        if bytes.len() < 1 + NONCE_LEN || bytes[0] != FORMAT_V1 {
            return Err(AppError::Internal("unsupported sealed format".into()));
        }
        let (nonce, ciphertext) = bytes[1..].split_at(NONCE_LEN);
        let nonce = XNonce::try_from(nonce).map_err(|_| AppError::Internal("bad nonce".into()))?;
        self.cipher
            .decrypt(&nonce, ciphertext)
            .map_err(|_| AppError::Internal("sealed value could not be opened".into()))
    }

    fn hash_password(&self, password: &Secret) -> AppResult<String> {
        let mut salt = [0u8; 16];
        rand::rng().fill(&mut salt[..]);
        PasswordHasher::<PasswordHash>::hash_password_with_salt(
            &Argon2::default(),
            password.expose().as_bytes(),
            &salt,
        )
        .map(|hash| hash.to_string())
        .map_err(|_| AppError::Internal("password hashing failed".into()))
    }

    fn verify_password(&self, password: &Secret, phc: &str) -> bool {
        PasswordHash::new(phc)
            .map(|parsed| {
                Argon2::default()
                    .verify_password(password.expose().as_bytes(), &parsed)
                    .is_ok()
            })
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_round_trip_and_tamper_detection() -> Result<(), Box<dyn std::error::Error>> {
        let crypto = MasterKeyCrypto::from_key(&[7u8; 32])?;
        let sealed = crypto.seal(b"secret token")?;
        assert_ne!(&sealed.0[..], b"secret token");
        assert_eq!(crypto.open(&sealed)?, b"secret token");
        let mut tampered = sealed.clone();
        if let Some(last) = tampered.0.last_mut() {
            *last ^= 1;
        }
        assert!(crypto.open(&tampered).is_err());
        let other = MasterKeyCrypto::from_key(&[8u8; 32])?;
        assert!(other.open(&sealed).is_err());
        Ok(())
    }

    #[test]
    fn password_hash_verifies() -> Result<(), Box<dyn std::error::Error>> {
        let crypto = MasterKeyCrypto::from_key(&[1u8; 32])?;
        let phc = crypto.hash_password(&Secret("correct horse".into()))?;
        assert!(phc.starts_with("$argon2id$"));
        assert!(crypto.verify_password(&Secret("correct horse".into()), &phc));
        assert!(!crypto.verify_password(&Secret("wrong".into()), &phc));
        assert!(!crypto.verify_password(&Secret("x".into()), "not a hash"));
        Ok(())
    }

    #[test]
    fn key_file_is_created_private_and_reloaded() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("master.key");
        let a = MasterKeyCrypto::load_or_create(&path)?;
        let mode = fs::metadata(&path)?.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let sealed = a.seal(b"x")?;
        let b = MasterKeyCrypto::load_or_create(&path)?;
        assert_eq!(b.open(&sealed)?, b"x");
        assert_eq!(
            a.sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        Ok(())
    }
}
