//! GitHub App JWT (RS256) signing with `ring`.
//!
//! Choice: `ring` directly instead of `jsonwebtoken`. `jsonwebtoken`'s `rust_crypto` backend
//! depends on the `rsa` crate, which carries RUSTSEC-2023-0071 (Marvin timing side channel,
//! no patched release) and fails `cargo deny`; `ring` is already the TLS provider, signs
//! RSA in constant time and keeps the crypto supply chain to one crate.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
use yulius_domain::time::Timestamp;

use crate::config::AppCredentials;
use crate::error::GitHubError;
use crate::secret::Secret;

/// JWT `iat` backdating to tolerate clock drift (GitHub's recommendation).
pub(crate) const IAT_BACKDATE_SECS: i64 = 60;
/// JWT lifetime after `now`; with the backdate the token spans exactly 10 minutes, the
/// maximum GitHub accepts.
pub(crate) const EXP_AFTER_SECS: i64 = 9 * 60;

/// A parsed App key plus the JWT issuer.
pub(crate) struct AppSigner {
    key: RsaKeyPair,
    issuer: String,
    rng: SystemRandom,
}

impl std::fmt::Debug for AppSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppSigner")
            .field("issuer", &self.issuer)
            .finish_non_exhaustive()
    }
}

/// Decodes the first PEM block, returning `(is_pkcs8, der)`.
pub(crate) fn pem_to_der(pem: &str) -> Option<(bool, Vec<u8>)> {
    let (label, begin_len) = if let Some(start) = pem.find("-----BEGIN RSA PRIVATE KEY-----") {
        (start, "-----BEGIN RSA PRIVATE KEY-----".len())
    } else {
        let start = pem.find("-----BEGIN PRIVATE KEY-----")?;
        (start, "-----BEGIN PRIVATE KEY-----".len())
    };
    let pkcs8 = !pem[label..].starts_with("-----BEGIN RSA");
    let body_start = label + begin_len;
    let body_end = body_start + pem[body_start..].find("-----END")?;
    let body: String = pem[body_start..body_end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    STANDARD.decode(body).ok().map(|der| (pkcs8, der))
}

/// Parses a PEM RSA private key for signing.
pub(crate) fn parse_rsa_key(pem: &str) -> Result<RsaKeyPair, GitHubError> {
    let (pkcs8, der) = pem_to_der(pem).ok_or(GitHubError::InvalidKey)?;
    let parsed = if pkcs8 {
        RsaKeyPair::from_pkcs8(&der)
    } else {
        RsaKeyPair::from_der(&der)
    };
    parsed.map_err(|_| GitHubError::InvalidKey)
}

impl AppSigner {
    pub(crate) fn new(app: &AppCredentials) -> Result<Self, GitHubError> {
        let key = parse_rsa_key(app.private_key_pem.expose())?;
        let issuer = if app.client_id.is_empty() {
            app.app_id.to_string()
        } else {
            app.client_id.clone()
        };
        Ok(Self {
            key,
            issuer,
            rng: SystemRandom::new(),
        })
    }

    /// Signs `{"iat": now-60s, "exp": now+9min, "iss": <client id>}`.
    pub(crate) fn sign(&self, now: Timestamp) -> Result<Secret, GitHubError> {
        let now_secs = now.0.div_euclid(1000);
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        let claims = serde_json::json!({
            "iat": now_secs - IAT_BACKDATE_SECS,
            "exp": now_secs + EXP_AFTER_SECS,
            "iss": self.issuer,
        });
        let claims = URL_SAFE_NO_PAD.encode(claims.to_string());
        let signing_input = format!("{header}.{claims}");
        let mut signature = vec![0_u8; self.key.public().modulus_len()];
        self.key
            .sign(
                &RSA_PKCS1_SHA256,
                &self.rng,
                signing_input.as_bytes(),
                &mut signature,
            )
            .map_err(|_| GitHubError::InvalidKey)?;
        Ok(Secret::new(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature)
        )))
    }
}

/// Verifies an RS256 JWT against the public half of `key` and returns its claims.
#[cfg(any(test, feature = "fake"))]
pub(crate) fn verify(token: &str, key: &RsaKeyPair) -> Option<serde_json::Value> {
    use ring::signature::{KeyPair, RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
    let (signing_input, signature) = token.rsplit_once('.')?;
    let (header, claims) = signing_input.split_once('.')?;
    let header: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).ok()?).ok()?;
    if header.get("alg")?.as_str()? != "RS256" {
        return None;
    }
    let signature = URL_SAFE_NO_PAD.decode(signature).ok()?;
    UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, key.public_key().as_ref())
        .verify(signing_input.as_bytes(), &signature)
        .ok()?;
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = include_str!("fake/synthetic_app_key.pem");

    fn credentials(pem: &str) -> AppCredentials {
        AppCredentials {
            app_id: 42,
            client_id: "Iv1.synthetic".into(),
            client_secret: Secret::new("secret"),
            private_key_pem: Secret::new(pem),
            slug: "yulius-test".into(),
            webhook_secret: None,
        }
    }

    #[test]
    fn signs_verifiable_ten_minute_tokens() -> Result<(), GitHubError> {
        let signer = AppSigner::new(&credentials(KEY))?;
        let token = signer.sign(Timestamp(1_700_000_000_000))?;
        let key = parse_rsa_key(KEY)?;
        let claims = verify(token.expose(), &key).ok_or(GitHubError::InvalidKey)?;
        assert_eq!(claims["iss"], "Iv1.synthetic");
        assert_eq!(claims["iat"], 1_700_000_000 - 60);
        assert_eq!(claims["exp"], 1_700_000_000 + 540);
        // Tampering breaks the signature.
        let tampered = token.expose().replacen('.', ".x", 1);
        assert!(verify(&tampered, &key).is_none());
        Ok(())
    }

    #[test]
    fn rejects_unusable_keys() {
        assert!(matches!(
            AppSigner::new(&credentials("not a key")),
            Err(GitHubError::InvalidKey)
        ));
        let truncated = "-----BEGIN RSA PRIVATE KEY-----\nMIIBOgIBAAJBAKj34GkxFhD9\n-----END RSA PRIVATE KEY-----\n";
        assert!(matches!(
            AppSigner::new(&credentials(truncated)),
            Err(GitHubError::InvalidKey)
        ));
    }
}
