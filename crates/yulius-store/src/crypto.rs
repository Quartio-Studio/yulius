//! Crypto adapter: master-key sealing (XChaCha20-Poly1305), the vault's envelope encryption
//! (per-secret data keys wrapped by a key derived from the master key with HKDF-SHA256), TOTP
//! HMACs, Argon2id password hashes, SHA-256, constant-time comparison and random identifiers
//! (SPEC-002 §8, ADR-18).

use std::fs;
use std::io::Write;
use std::path::Path;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use rand::RngExt as _;
use sha2::{Digest, Sha256};
use yulius_app::error::{AppError, AppResult};
use subtle::ConstantTimeEq as _;
use yulius_app::model::{Sealed, Secret};
use yulius_app::ports::{Crypto, Envelope};
use yulius_domain::vault::totp::OtpAlgorithm;
use zeroize::Zeroizing;

/// Ciphertext format version byte.
const FORMAT_V1: u8 = 1;
const NONCE_LEN: usize = 24;
/// Generation of the vault key-encryption key (HKDF info below); recorded with each envelope.
pub const VAULT_KEK_ID: u32 = 1;
/// HKDF info deriving the vault key-encryption key from the master key (domain separation from
/// every other use of the master key).
const VAULT_KEK_INFO: &[u8] = b"yulius/vault/kek/v1";
/// Associated-data prefixes: a wrapped data key and a payload never open as each other.
const AAD_DATA_KEY: &[u8] = b"yulius-vault-dek-v1|";
const AAD_PAYLOAD: &[u8] = b"yulius-vault-data-v1|";

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
    vault_kek: XChaCha20Poly1305,
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
        let kek = derive_vault_kek(key)?;
        let key = Key::try_from(key).map_err(|_| KeyError::BadLength)?;
        let kek = Key::try_from(&kek[..]).map_err(|_| KeyError::BadLength)?;
        Ok(Self {
            cipher: XChaCha20Poly1305::new(&key),
            vault_kek: XChaCha20Poly1305::new(&kek),
        })
    }
}

/// HKDF-SHA256 (RFC 5869, `ring`) of the master key with [`VAULT_KEK_INFO`].
fn derive_vault_kek(master: &[u8]) -> Result<Zeroizing<[u8; 32]>, KeyError> {
    let prk = ring::hkdf::Salt::new(ring::hkdf::HKDF_SHA256, &[]).extract(master);
    let info = [VAULT_KEK_INFO];
    let okm = prk
        .expand(&info, ring::hkdf::HKDF_SHA256)
        .map_err(|_| KeyError::BadLength)?;
    let mut out = Zeroizing::new([0u8; 32]);
    okm.fill(&mut out[..]).map_err(|_| KeyError::BadLength)?;
    Ok(out)
}

fn with_prefix(prefix: &[u8], aad: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(prefix.len() + aad.len());
    out.extend_from_slice(prefix);
    out.extend_from_slice(aad);
    out
}

/// Seals with a random nonce: `nonce || ciphertext || tag`.
fn seal_with(cipher: &XChaCha20Poly1305, aad: &[u8], plaintext: &[u8]) -> AppResult<Vec<u8>> {
    let mut nonce = [0u8; NONCE_LEN];
    rand::rng().fill(&mut nonce[..]);
    let ciphertext = cipher
        .encrypt(&XNonce::from(nonce), Payload { msg: plaintext, aad })
        .map_err(|_| AppError::Internal("vault seal failed".into()))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn open_with(
    cipher: &XChaCha20Poly1305,
    aad: &[u8],
    sealed: &[u8],
) -> AppResult<Zeroizing<Vec<u8>>> {
    if sealed.len() < NONCE_LEN {
        return Err(AppError::Internal("vault value could not be opened".into()));
    }
    let (nonce, ciphertext) = sealed.split_at(NONCE_LEN);
    let nonce = XNonce::try_from(nonce)
        .map_err(|_| AppError::Internal("vault value could not be opened".into()))?;
    cipher
        .decrypt(&nonce, Payload { msg: ciphertext, aad })
        .map(Zeroizing::new)
        .map_err(|_| AppError::Internal("vault value could not be opened".into()))
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

    fn envelope_seal(&self, aad: &[u8], plaintext: &[u8]) -> AppResult<Envelope> {
        let mut data_key = Zeroizing::new([0u8; 32]);
        rand::rng().fill(&mut data_key[..]);
        let key = Key::try_from(&data_key[..])
            .map_err(|_| AppError::Internal("vault key".into()))?;
        let cipher = XChaCha20Poly1305::new(&key);
        let ciphertext = seal_with(&cipher, &with_prefix(AAD_PAYLOAD, aad), plaintext)?;
        let wrapped_key = seal_with(
            &self.vault_kek,
            &with_prefix(AAD_DATA_KEY, aad),
            &data_key[..],
        )?;
        Ok(Envelope {
            kek_id: VAULT_KEK_ID,
            wrapped_key,
            ciphertext,
        })
    }

    fn envelope_open(&self, aad: &[u8], envelope: &Envelope) -> AppResult<Zeroizing<Vec<u8>>> {
        if envelope.kek_id != VAULT_KEK_ID {
            return Err(AppError::Internal(
                "vault value sealed with an unknown key generation".into(),
            ));
        }
        let data_key = open_with(
            &self.vault_kek,
            &with_prefix(AAD_DATA_KEY, aad),
            &envelope.wrapped_key,
        )?;
        let key = Key::try_from(&data_key[..])
            .map_err(|_| AppError::Internal("vault value could not be opened".into()))?;
        let cipher = XChaCha20Poly1305::new(&key);
        open_with(&cipher, &with_prefix(AAD_PAYLOAD, aad), &envelope.ciphertext)
    }

    fn totp_hmac(&self, algorithm: OtpAlgorithm, key: &[u8], counter: u64) -> Vec<u8> {
        let algorithm = match algorithm {
            OtpAlgorithm::Sha1 => ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY,
            OtpAlgorithm::Sha256 => ring::hmac::HMAC_SHA256,
            OtpAlgorithm::Sha512 => ring::hmac::HMAC_SHA512,
        };
        let key = ring::hmac::Key::new(algorithm, key);
        ring::hmac::sign(&key, &counter.to_be_bytes())
            .as_ref()
            .to_vec()
    }

    fn constant_time_eq(&self, a: &[u8], b: &[u8]) -> bool {
        a.len() == b.len() && bool::from(a.ct_eq(b))
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
    fn envelopes_open_only_with_their_row_and_key() -> Result<(), Box<dyn std::error::Error>> {
        let crypto = MasterKeyCrypto::from_key(&[7u8; 32])?;
        let envelope = crypto.envelope_seal(b"secret-1|team", b"hunter2 password")?;
        assert_eq!(envelope.kek_id, VAULT_KEK_ID);
        assert!(
            !envelope
                .ciphertext
                .windows(8)
                .any(|w| w == b"hunter2 ")
        );
        assert_eq!(
            crypto.envelope_open(b"secret-1|team", &envelope)?.as_slice(),
            b"hunter2 password"
        );
        // Another row's identity, a flipped bit, another master key: nothing opens.
        assert!(crypto.envelope_open(b"secret-2|team", &envelope).is_err());
        let mut tampered = envelope.clone();
        if let Some(byte) = tampered.ciphertext.last_mut() {
            *byte ^= 1;
        }
        assert!(crypto.envelope_open(b"secret-1|team", &tampered).is_err());
        let mut swapped = envelope.clone();
        swapped.wrapped_key = crypto.envelope_seal(b"secret-1|team", b"x")?.wrapped_key;
        assert!(crypto.envelope_open(b"secret-1|team", &swapped).is_err());
        let other = MasterKeyCrypto::from_key(&[8u8; 32])?;
        assert!(other.envelope_open(b"secret-1|team", &envelope).is_err());
        // Each seal uses a fresh data key and nonce.
        let again = crypto.envelope_seal(b"secret-1|team", b"hunter2 password")?;
        assert_ne!(again.ciphertext, envelope.ciphertext);
        assert_ne!(again.wrapped_key, envelope.wrapped_key);
        Ok(())
    }

    /// RFC 6238 appendix B, 8 digits: SHA-1, SHA-256 and SHA-512 at T = 59 s and 1111111109 s.
    #[test]
    fn totp_matches_rfc_6238() -> Result<(), Box<dyn std::error::Error>> {
        use yulius_domain::vault::totp::{code_from_hmac, counter};
        let crypto = MasterKeyCrypto::from_key(&[1u8; 32])?;
        let sha1 = b"12345678901234567890".to_vec();
        let sha256 = b"12345678901234567890123456789012".to_vec();
        let sha512 = b"1234567890123456789012345678901234567890123456789012345678901234".to_vec();
        for (seconds, codes) in [
            (59_i64, ["94287082", "46119246", "90693936"]),
            (1_111_111_109, ["07081804", "68084774", "25091201"]),
        ] {
            let t = counter(seconds * 1000, 30);
            for ((algorithm, key), code) in [
                (OtpAlgorithm::Sha1, &sha1),
                (OtpAlgorithm::Sha256, &sha256),
                (OtpAlgorithm::Sha512, &sha512),
            ]
            .into_iter()
            .zip(codes)
            {
                assert_eq!(code_from_hmac(&crypto.totp_hmac(algorithm, key, t), 8), code);
            }
        }
        Ok(())
    }

    #[test]
    fn constant_time_equality() -> Result<(), Box<dyn std::error::Error>> {
        let crypto = MasterKeyCrypto::from_key(&[1u8; 32])?;
        assert!(crypto.constant_time_eq(b"abc", b"abc"));
        assert!(!crypto.constant_time_eq(b"abc", b"abd"));
        assert!(!crypto.constant_time_eq(b"abc", b"abcd"));
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
