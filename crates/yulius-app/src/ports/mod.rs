//! Ports implemented by adapters and injected at composition.

pub mod browser;
pub mod egress;
pub mod git;
pub mod github;
pub mod harness;
pub mod intake;
pub mod session;
pub mod session_docs;
pub mod store;
pub mod store_ext;
pub mod system;
pub mod updates;
pub mod usage;
pub mod vault;

use yulius_domain::time::Timestamp;

use crate::error::AppResult;
use crate::model::{Sealed, Secret};

/// A vault payload under envelope encryption (SPEC-002 ADR-20): the payload sealed with its own
/// random data key, and that key wrapped by the vault key-encryption key `kek_id`.
#[derive(Clone, PartialEq, Eq)]
pub struct Envelope {
    /// Which key-encryption key wrapped the data key.
    pub kek_id: u32,
    /// The wrapped data key (nonce, ciphertext and tag).
    pub wrapped_key: Vec<u8>,
    /// The sealed payload (nonce, ciphertext and tag).
    pub ciphertext: Vec<u8>,
}

impl std::fmt::Debug for Envelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Envelope(kek {}, {} + {} bytes)",
            self.kek_id,
            self.wrapped_key.len(),
            self.ciphertext.len()
        )
    }
}

/// Injected clock.
pub trait Clock: Send + Sync {
    /// Current time.
    fn now(&self) -> Timestamp;
}

/// Randomness, hashing and secret sealing.
pub trait Crypto: Send + Sync {
    /// A new random identifier (UUID v4 text).
    fn new_id(&self) -> String;
    /// 32 random bytes.
    fn random_bytes(&self) -> [u8; 32];
    /// Lowercase hex SHA-256.
    fn sha256_hex(&self, data: &[u8]) -> String;
    /// Encrypts with the instance master key.
    ///
    /// # Errors
    /// Fails when the cipher fails.
    fn seal(&self, plaintext: &[u8]) -> AppResult<Sealed>;
    /// Decrypts with the instance master key.
    ///
    /// # Errors
    /// Fails for tampered or foreign ciphertext.
    fn open(&self, sealed: &Sealed) -> AppResult<Vec<u8>>;
    /// Argon2id PHC hash of a password.
    ///
    /// # Errors
    /// Fails when hashing fails.
    fn hash_password(&self, password: &Secret) -> AppResult<String>;
    /// Verifies a password against a PHC hash (constant time).
    fn verify_password(&self, password: &Secret, phc: &str) -> bool;
    /// Envelope encryption (vault): seals `plaintext` under a fresh random data key bound to
    /// `aad` (the row's identity), and wraps the data key with the vault key-encryption key.
    ///
    /// # Errors
    /// Fails when the cipher fails.
    fn envelope_seal(&self, aad: &[u8], plaintext: &[u8]) -> AppResult<Envelope>;
    /// Opens an [`Envelope`] sealed with the same `aad`; the plaintext is zeroed when dropped.
    ///
    /// # Errors
    /// Fails for tampered, moved (other `aad`) or foreign envelopes.
    fn envelope_open(
        &self,
        aad: &[u8],
        envelope: &Envelope,
    ) -> AppResult<zeroize::Zeroizing<Vec<u8>>>;
    /// HMAC of a TOTP counter (RFC 6238) with `key`.
    fn totp_hmac(
        &self,
        algorithm: yulius_domain::vault::totp::OtpAlgorithm,
        key: &[u8],
        counter: u64,
    ) -> Vec<u8>;
    /// Constant-time equality of two byte strings (secret material).
    fn constant_time_eq(&self, a: &[u8], b: &[u8]) -> bool;
}
