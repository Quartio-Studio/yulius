//! The egress certificate authority (SPEC-002 ADR-18): one ECDSA P-256 CA per instance, trusted
//! only by agent runs (through the run's CA bundle), signing short-lived leaf certificates for
//! the hosts bound to agent secrets. Its key is sealed in the vault (`vault_meta`), never on
//! disk in clear.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, date_time_ymd,
};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde_json::json;

/// Leaf certificates are reissued after this long (they are valid a few days longer).
const LEAF_REUSE: Duration = Duration::from_secs(5 * 24 * 3600);
/// Cached leaf configurations.
const LEAF_CACHE_MAX: usize = 512;

/// Errors creating or loading the CA, or issuing a leaf.
#[derive(Debug, thiserror::Error)]
#[error("egress certificate authority: {0}")]
pub struct CaError(String);

fn err(context: &str, cause: impl std::fmt::Display) -> CaError {
    CaError(format!("{context}: {cause}"))
}

/// Civil date of `days` after 1970-01-01 (proleptic Gregorian).
fn civil(days: i64) -> (i32, u8, u8) {
    let (y, m, d) = yulius_app::auth::civil_from_days(days);
    (
        i32::try_from(y).unwrap_or(1970),
        u8::try_from(m).unwrap_or(1),
        u8::try_from(d).unwrap_or(1),
    )
}

fn today() -> i64 {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    i64::try_from(secs / 86_400).unwrap_or(0)
}

fn ca_params(name: &str) -> CertificateParams {
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, name);
    dn.push(DnType::OrganizationName, "Yulius");
    params.distinguished_name = dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params
}

/// Generates a new CA; returns its material (JSON with the certificate, the key and the name)
/// for sealing in the vault.
///
/// # Errors
/// Key generation or signing failures.
pub fn generate_ca_material() -> Result<String, CaError> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() ^ u32::try_from(d.as_secs() % 0xffff_ffff).unwrap_or(0))
        .unwrap_or(0);
    let name = format!("Yulius egress CA {nanos:08x}");
    let key = KeyPair::generate().map_err(|e| err("key", e))?;
    let mut params = ca_params(&name);
    let start = today() - 1;
    let ((y0, m0, d0), (y1, m1, d1)) = (civil(start), civil(start + 3650));
    params.not_before = date_time_ymd(y0, m0, d0);
    params.not_after = date_time_ymd(y1, m1, d1);
    let cert = params
        .self_signed(&key)
        .map_err(|e| err("self-signed", e))?;
    Ok(json!({"name": name, "cert_pem": cert.pem(), "key_pem": key.serialize_pem()}).to_string())
}

/// The loaded CA: it issues leaf certificates and their TLS configurations.
pub struct CertificateAuthority {
    issuer: Issuer<'static, KeyPair>,
    cert_pem: String,
    cert_der: CertificateDer<'static>,
    leaves: Mutex<HashMap<String, (Arc<ServerConfig>, Instant)>>,
}

impl std::fmt::Debug for CertificateAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertificateAuthority")
            .finish_non_exhaustive()
    }
}

impl CertificateAuthority {
    /// Loads material made by [`generate_ca_material`].
    ///
    /// # Errors
    /// Malformed material.
    pub fn from_material(material: &str) -> Result<Self, CaError> {
        let value: serde_json::Value =
            serde_json::from_str(material).map_err(|e| err("material", e))?;
        let field = |name: &str| {
            value
                .get(name)
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| CaError(format!("material without {name}")))
        };
        let name = field("name")?;
        let cert_pem = field("cert_pem")?;
        let key = KeyPair::from_pem(&field("key_pem")?).map_err(|e| err("key", e))?;
        let cert_der = pem_certificates(&cert_pem)
            .into_iter()
            .next()
            .ok_or_else(|| CaError("material without a certificate".into()))?;
        Ok(Self {
            issuer: Issuer::new(ca_params(&name), key),
            cert_pem,
            cert_der,
            leaves: Mutex::new(HashMap::new()),
        })
    }

    /// The CA certificate (PEM) that agent runs trust.
    #[must_use]
    pub fn cert_pem(&self) -> &str {
        &self.cert_pem
    }

    /// A TLS server configuration presenting a leaf certificate for `host` (HTTP/1.1 only),
    /// cached for a few days.
    ///
    /// # Errors
    /// Key generation, signing or TLS configuration failures.
    pub fn server_config(&self, host: &str) -> Result<Arc<ServerConfig>, CaError> {
        let host = host.to_ascii_lowercase();
        if let Ok(leaves) = self.leaves.lock()
            && let Some((config, at)) = leaves.get(&host)
            && at.elapsed() < LEAF_REUSE
        {
            return Ok(config.clone());
        }
        let key = KeyPair::generate().map_err(|e| err("leaf key", e))?;
        let mut params =
            CertificateParams::new(vec![host.clone()]).map_err(|e| err("leaf name", e))?;
        params
            .distinguished_name
            .push(DnType::CommonName, host.as_str());
        let start = today() - 1;
        let ((y0, m0, d0), (y1, m1, d1)) = (civil(start), civil(start + 8));
        params.not_before = date_time_ymd(y0, m0, d0);
        params.not_after = date_time_ymd(y1, m1, d1);
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.use_authority_key_identifier_extension = true;
        let cert = params
            .signed_by(&key, &self.issuer)
            .map_err(|e| err("leaf", e))?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| err("tls", e))?
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.der().clone(), self.cert_der.clone()],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            )
            .map_err(|e| err("tls", e))?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        let config = Arc::new(config);
        if let Ok(mut leaves) = self.leaves.lock() {
            if leaves.len() >= LEAF_CACHE_MAX {
                leaves.clear();
            }
            leaves.insert(host, (config.clone(), Instant::now()));
        }
        Ok(config)
    }
}

/// DER certificates of a PEM text.
#[must_use]
pub fn pem_certificates(pem: &str) -> Vec<CertificateDer<'static>> {
    use base64::Engine as _;
    let mut out = Vec::new();
    let mut inside = false;
    let mut body = String::new();
    for line in pem.lines() {
        let line = line.trim();
        if line == "-----BEGIN CERTIFICATE-----" {
            inside = true;
            body.clear();
        } else if line == "-----END CERTIFICATE-----" {
            inside = false;
            if let Ok(der) = base64::engine::general_purpose::STANDARD.decode(body.as_bytes()) {
                out.push(CertificateDer::from(der));
            }
        } else if inside {
            body.push_str(line);
        }
    }
    out
}

/// Where hosts keep their trusted roots (Debian/Ubuntu, Fedora/RHEL, Alpine/macOS, SUSE).
const SYSTEM_BUNDLES: &[&str] = &[
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/cert.pem",
    "/etc/ssl/ca-bundle.pem",
];

/// The CA bundle agent runs use: the host's trusted roots plus the egress CA. `None` when the
/// host's roots are not found: replacing a client's roots with the egress CA alone would break
/// every other HTTPS connection.
#[must_use]
pub fn run_ca_bundle(ca_pem: &str) -> Option<String> {
    let roots = SYSTEM_BUNDLES
        .iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .filter(|text| text.contains("BEGIN CERTIFICATE"))?;
    let mut bundle = roots;
    if !bundle.ends_with('\n') {
        bundle.push('\n');
    }
    bundle.push_str("# Yulius egress CA (agent secrets, SPEC-002 ADR-18)\n");
    bundle.push_str(ca_pem);
    Some(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issues_cached_leaves_and_reloads() -> Result<(), Box<dyn std::error::Error>> {
        let material = generate_ca_material()?;
        let ca = CertificateAuthority::from_material(&material)?;
        assert!(ca.cert_pem().contains("BEGIN CERTIFICATE"));
        let a = ca.server_config("API.example.com")?;
        let b = ca.server_config("api.example.com")?;
        assert!(Arc::ptr_eq(&a, &b), "cached per host");
        assert_eq!(a.alpn_protocols, vec![b"http/1.1".to_vec()]);
        let again = CertificateAuthority::from_material(&material)?;
        assert_eq!(again.cert_pem(), ca.cert_pem());
        assert!(CertificateAuthority::from_material("{}").is_err());
        assert_eq!(pem_certificates(ca.cert_pem()).len(), 1);
        Ok(())
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_723), (2024, 1, 1));
    }
}
