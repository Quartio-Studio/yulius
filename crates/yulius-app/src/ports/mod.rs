//! Ports implemented by adapters and injected at composition.

pub mod browser;
pub mod editor;
pub mod git;
pub mod github;
pub mod harness;
pub mod session;
pub mod store;
pub mod store_ext;

use yulius_domain::time::Timestamp;

use crate::error::AppResult;
use crate::model::{Sealed, Secret};

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
}
