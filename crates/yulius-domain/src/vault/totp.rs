//! Time-based one-time passwords (RFC 6238 over RFC 4226): seed parsing (base32 or an
//! `otpauth://totp/…` URI), the time counter and dynamic truncation. The HMAC is computed by the
//! caller's crypto adapter.

use super::VaultError;

/// HMAC algorithm of a TOTP seed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtpAlgorithm {
    /// HMAC-SHA-1 (the default and by far the most common).
    Sha1,
    /// HMAC-SHA-256.
    Sha256,
    /// HMAC-SHA-512.
    Sha512,
}

/// A parsed TOTP seed.
#[derive(Clone, PartialEq, Eq)]
pub struct OtpConfig {
    /// The shared secret.
    pub secret: Vec<u8>,
    /// Code length (6–8).
    pub digits: u32,
    /// Step in seconds (15–300).
    pub period: u64,
    /// HMAC algorithm.
    pub algorithm: OtpAlgorithm,
}

impl std::fmt::Debug for OtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OtpConfig")
            .field("secret", &"<redacted>")
            .field("digits", &self.digits)
            .field("period", &self.period)
            .field("algorithm", &self.algorithm)
            .finish()
    }
}

/// Decodes RFC 4648 base32 (case-insensitive; spaces, dashes and `=` padding ignored).
///
/// # Errors
/// [`VaultError::Otp`] for a character outside the alphabet.
pub fn base32_decode(text: &str) -> Result<Vec<u8>, VaultError> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for c in text.chars().filter(|c| !matches!(c, ' ' | '-' | '=')) {
        let value = match c.to_ascii_uppercase() {
            c @ 'A'..='Z' => u32::from(c) - u32::from('A'),
            c @ '2'..='7' => u32::from(c) - u32::from('2') + 26,
            _ => {
                return Err(VaultError::Otp(
                    "the TOTP seed is not base32 (letters A–Z and digits 2–7)".into(),
                ));
            }
        };
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((buffer >> bits) & 0xff).unwrap_or(0));
        }
    }
    Ok(out)
}

/// RFC 4648 base32 of `bytes`, uppercase and without padding (what authenticator apps take).
#[must_use]
pub fn base32_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &byte in bytes {
        buffer = ((buffer << 8) | u32::from(byte)) & 0xffff;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(ALPHABET[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(ALPHABET[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

/// The `otpauth://totp/…` link of a base32 seed with the defaults (SHA-1, 6 digits, 30 s): what
/// an authenticator app's QR code holds.
#[must_use]
pub fn otpauth_uri(issuer: &str, account: &str, seed: &str) -> String {
    use super::encode::percent;
    let issuer = percent(issuer.as_bytes());
    format!(
        "otpauth://totp/{issuer}:{}?secret={seed}&issuer={issuer}&algorithm=SHA1&digits=6&period=30",
        percent(account.as_bytes())
    )
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(Ok(b)) = text.get(i + 1..i + 3).map(|h| u8::from_str_radix(h, 16))
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parses a seed: plain base32 or an `otpauth://totp/<label>?secret=…[&digits=&period=&algorithm=]`
/// URI (the content of an authenticator QR code).
///
/// # Errors
/// [`VaultError::Otp`] for HOTP URIs, missing or short secrets and unsupported parameters.
pub fn parse_otp(text: &str) -> Result<OtpConfig, VaultError> {
    let text = text.trim();
    let mut config = OtpConfig {
        secret: Vec::new(),
        digits: 6,
        period: 30,
        algorithm: OtpAlgorithm::Sha1,
    };
    if let Some(rest) = text
        .get(..10)
        .filter(|p| p.eq_ignore_ascii_case("otpauth://"))
        .map(|_| &text[10..])
    {
        let (kind, rest) = rest.split_once('/').unwrap_or((rest, ""));
        if !kind.eq_ignore_ascii_case("totp") {
            return Err(VaultError::Otp(
                "only time-based codes (otpauth://totp/…) are supported".into(),
            ));
        }
        let query = rest.split_once('?').map_or("", |(_, q)| q);
        let mut secret = None;
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            let value = percent_decode(value);
            match key.to_ascii_lowercase().as_str() {
                "secret" => secret = Some(base32_decode(&value)?),
                "digits" => {
                    config.digits = value
                        .parse()
                        .ok()
                        .filter(|d| (6..=8).contains(d))
                        .ok_or_else(|| VaultError::Otp("digits must be 6, 7 or 8".into()))?;
                }
                "period" => {
                    config.period = value
                        .parse()
                        .ok()
                        .filter(|p| (15..=300).contains(p))
                        .ok_or_else(|| VaultError::Otp("period must be 15–300 seconds".into()))?;
                }
                "algorithm" => {
                    config.algorithm = match value.to_ascii_uppercase().as_str() {
                        "SHA1" => OtpAlgorithm::Sha1,
                        "SHA256" => OtpAlgorithm::Sha256,
                        "SHA512" => OtpAlgorithm::Sha512,
                        _ => {
                            return Err(VaultError::Otp(
                                "algorithm must be SHA1, SHA256 or SHA512".into(),
                            ));
                        }
                    };
                }
                _ => {}
            }
        }
        config.secret = secret.ok_or_else(|| VaultError::Otp("the URI has no secret".into()))?;
    } else {
        config.secret = base32_decode(text)?;
    }
    if config.secret.len() < 10 {
        return Err(VaultError::Otp(
            "the TOTP seed is too short (at least 16 base32 characters)".into(),
        ));
    }
    Ok(config)
}

/// The moving factor at `unix_ms` for `period` seconds.
#[must_use]
pub fn counter(unix_ms: i64, period: u64) -> u64 {
    let seconds = u64::try_from(unix_ms.max(0) / 1000).unwrap_or(0);
    seconds / period.max(1)
}

/// Seconds until the code at `unix_ms` changes.
#[must_use]
pub fn remaining_seconds(unix_ms: i64, period: u64) -> u64 {
    let seconds = u64::try_from(unix_ms.max(0) / 1000).unwrap_or(0);
    let period = period.max(1);
    period - seconds % period
}

/// The code from an HMAC of the counter (RFC 4226 dynamic truncation), zero-padded.
#[must_use]
pub fn code_from_hmac(hmac: &[u8], digits: u32) -> String {
    let Some(&last) = hmac.last() else {
        return String::new();
    };
    let offset = usize::from(last & 0x0f);
    let Some(window) = hmac.get(offset..offset + 4) else {
        return String::new();
    };
    let binary = (u32::from(window[0] & 0x7f) << 24)
        | (u32::from(window[1]) << 16)
        | (u32::from(window[2]) << 8)
        | u32::from(window[3]);
    let code = binary % 10u32.pow(digits);
    format!(
        "{code:0width$}",
        width = usize::try_from(digits).unwrap_or(6)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .filter_map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
            .collect()
    }

    /// RFC 4226 appendix D: HMAC-SHA-1 values of counters 0..9 for "12345678901234567890" and
    /// the resulting 6-digit codes.
    #[test]
    fn truncation_matches_rfc_4226() {
        let vectors = [
            ("cc93cf18508d94934c64b65d8ba7667fb7cde4b0", "755224"),
            ("75a48a19d4cbe100644e8ac1397eea747a2d33ab", "287082"),
            ("0bacb7fa082fef30782211938bc1c5e70416ff44", "359152"),
            ("66c28227d03a2d5529262ff016a1e6ef76557ece", "969429"),
            ("a904c900a64b35909874b33e61c5938a8e15ed1c", "338314"),
            ("a37e783d7b7233c083d4f62926c7a25f238d0316", "254676"),
            ("bc9cd28561042c83f219324d3c607256c03272ae", "287922"),
            ("a4fb960c0bc06e1eabb804e5b397cdc4b45596fa", "162583"),
            ("1b3c89f65e6c9e883012052823443f048b4332db", "399871"),
            ("1637409809a679dc698207310c8c7fc07290d9e5", "520489"),
        ];
        for (hmac, code) in vectors {
            assert_eq!(code_from_hmac(&unhex(hmac), 6), code);
        }
    }

    #[test]
    fn base32_and_uris() -> Result<(), VaultError> {
        // "12345678901234567890" in base32.
        let seed = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        assert_eq!(base32_decode(seed)?, b"12345678901234567890".to_vec());
        assert_eq!(
            base32_decode("gezd gnbv gy3t qojq gezd gnbv gy3t qojq")?,
            b"12345678901234567890".to_vec()
        );
        assert!(base32_decode("GEZ1").is_err());
        let uri = parse_otp(&format!(
            "otpauth://totp/ACME%3Ateam%40acme.com?secret={seed}&issuer=ACME&digits=8&period=60&algorithm=SHA256"
        ))?;
        assert_eq!(uri.secret, b"12345678901234567890".to_vec());
        assert_eq!(
            (uri.digits, uri.period, uri.algorithm),
            (8, 60, OtpAlgorithm::Sha256)
        );
        assert!(parse_otp("otpauth://hotp/x?secret=GEZDGNBVGY3TQOJQ&counter=1").is_err());
        assert!(parse_otp("otpauth://totp/x?issuer=a").is_err());
        assert!(parse_otp("GEZDGNBV").is_err(), "too short");
        assert!(!format!("{uri:?}").contains("1234"), "Debug hides the seed");
        Ok(())
    }

    #[test]
    fn seeds_encode_to_base32_and_links_parse_back() -> Result<(), VaultError> {
        assert_eq!(
            base32_encode(b"12345678901234567890"),
            "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"
        );
        for len in 0..12 {
            let bytes: Vec<u8> = (0..len).map(|i: u8| i.wrapping_mul(37) ^ 0xa5).collect();
            assert_eq!(base32_decode(&base32_encode(&bytes))?, bytes, "{len} bytes");
        }
        let uri = otpauth_uri("Yulius", "anna smith", "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert!(
            uri.starts_with("otpauth://totp/Yulius:anna%20smith?"),
            "{uri}"
        );
        let parsed = parse_otp(&uri)?;
        assert_eq!(parsed.secret, b"12345678901234567890".to_vec());
        assert_eq!((parsed.digits, parsed.period), (6, 30));
        Ok(())
    }

    #[test]
    fn counters_follow_the_period() {
        assert_eq!(counter(59_000, 30), 1);
        assert_eq!(counter(1_111_111_109_000, 30), 37_037_036);
        assert_eq!(remaining_seconds(59_000, 30), 1);
        assert_eq!(remaining_seconds(60_000, 30), 30);
    }
}
