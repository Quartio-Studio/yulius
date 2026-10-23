//! Secrets vault rules (SPEC-001 VLT, SPEC-002 ADR-20): names and host bindings of agent
//! secrets, placeholders, the encodings a value is recognised in, exact-value redaction (plain
//! and fingerprinted), TOTP arithmetic, re-authentication freshness and the egress destination
//! policy. Pure functions only: hashing and HMAC are injected by callers.

pub mod dotenv;
pub mod encode;
pub mod hosts;
pub mod names;
pub mod ordinary;
pub mod plain;
pub mod redact;
pub mod totp;

use std::net::IpAddr;

use crate::time::Timestamp;

pub use hosts::HostPattern;
pub use names::SecretName;
pub use ordinary::{OrdinaryText, ordinary_text};

/// Errors validating vault input. Messages never contain secret values.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VaultError {
    /// A malformed or reserved secret name.
    #[error("{0}")]
    Name(String),
    /// A malformed host binding.
    #[error("{0}")]
    Host(String),
    /// A malformed header name.
    #[error("{0}")]
    Header(String),
    /// A value Yulius cannot hold for agents (too short, control characters, too long).
    #[error("{0}")]
    Value(String),
    /// A malformed TOTP seed or `otpauth://` URI.
    #[error("{0}")]
    Otp(String),
}

/// Shortest agent-secret value (characters): shorter values cannot be recognised in output
/// without redacting ordinary text, so they are refused (VLT-006).
pub const MIN_AGENT_VALUE_CHARS: usize = 8;
/// Longest stored field value in bytes (a token, a password, notes).
pub const MAX_FIELD_BYTES: usize = 16 * 1024;
/// How long a sign-in or re-authentication allows revealing team secrets (VLT-002).
pub const REAUTH_WINDOW_MS: i64 = 10 * 60 * 1000;
/// After this age the UI suggests rotating a value (VLT-001).
pub const ROTATE_AFTER_MS: i64 = 180 * 24 * 3600 * 1000;

/// Whether a session authenticated at `authenticated_at` may reveal team secrets at `now`.
#[must_use]
pub fn reauth_fresh(now: Timestamp, authenticated_at: Timestamp) -> bool {
    let age = now.millis_since(authenticated_at);
    (0..REAUTH_WINDOW_MS).contains(&age)
}

/// Checks an agent-secret value: at least [`MIN_AGENT_VALUE_CHARS`] characters, no control
/// characters (it must fit an HTTP header and never span lines), at most [`MAX_FIELD_BYTES`], and
/// not a key whose line breaks were lost ([`dotenv::flattened_key`], SPEC-193 MLK-002).
///
/// # Errors
/// [`VaultError::Value`] naming the rule.
pub fn check_agent_value(value: &str) -> Result<(), VaultError> {
    if value.chars().count() < MIN_AGENT_VALUE_CHARS {
        return Err(VaultError::Value(format!(
            "an agent secret needs at least {MIN_AGENT_VALUE_CHARS} characters: shorter values cannot be recognised and redacted"
        )));
    }
    if value.len() > MAX_FIELD_BYTES {
        return Err(VaultError::Value("the value is too long".into()));
    }
    if value.chars().any(char::is_control) {
        return Err(VaultError::Value(
            "an agent secret cannot contain line breaks or control characters".into(),
        ));
    }
    if dotenv::flattened_key(value) {
        return Err(VaultError::Value(
            "this key's line breaks were lost (a one-line field turns them into spaces): paste it into the Secrets page's Value field, which keeps them as \\n, or write each line break as \\n".into(),
        ));
    }
    Ok(())
}

/// How the egress proxy adds an agent secret to requests for its bound hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Injection {
    /// Only placeholders and browser filling.
    None,
    /// `Authorization: Bearer <value>`.
    Bearer,
    /// `Authorization: Basic base64(<username>:<value>)`.
    Basic,
    /// `<name>: <value>`.
    Header(String),
}

/// Header names the proxy never sets or lets a secret target.
const FORBIDDEN_HEADERS: &[&str] = &[
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    "upgrade",
    "te",
    "trailer",
    "keep-alive",
    "proxy-authorization",
    "proxy-connection",
    "x-yulius-secret",
];

impl Injection {
    /// Parses the stored form: `none`, `bearer`, `basic` or `header` with a header name.
    ///
    /// # Errors
    /// [`VaultError::Header`] for an unknown kind or a bad header name.
    pub fn parse(kind: &str, header: Option<&str>) -> Result<Self, VaultError> {
        match kind {
            "none" | "" => Ok(Self::None),
            "bearer" => Ok(Self::Bearer),
            "basic" => Ok(Self::Basic),
            "header" => {
                let name = header.unwrap_or_default().trim();
                let token = !name.is_empty()
                    && name.len() <= 64
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b));
                if !token || FORBIDDEN_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
                    return Err(VaultError::Header(format!(
                        "invalid header name {name:?}: letters, digits, - and _ (not a hop-by-hop or proxy header)"
                    )));
                }
                Ok(Self::Header(name.to_owned()))
            }
            other => Err(VaultError::Header(format!(
                "unknown injection {other:?}: none, bearer, basic or header"
            ))),
        }
    }

    /// The stored kind.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Bearer => "bearer",
            Self::Basic => "basic",
            Self::Header(_) => "header",
        }
    }

    /// The header this injection sets, if any (lowercase for comparisons).
    #[must_use]
    pub fn header_name(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Bearer | Self::Basic => Some("authorization".into()),
            Self::Header(name) => Some(name.to_ascii_lowercase()),
        }
    }

    /// A short description for people and agents, e.g. `Authorization: Bearer`.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::None => "no header (placeholders and browser filling only)".into(),
            Self::Bearer => "Authorization: Bearer <value>".into(),
            Self::Basic => "Authorization: Basic <username:value>".into(),
            Self::Header(name) => format!("{name}: <value>"),
        }
    }

    /// The header value for `value` (and `username` for Basic).
    #[must_use]
    pub fn render(&self, username: Option<&str>, value: &str) -> Option<(String, String)> {
        match self {
            Self::None => None,
            Self::Bearer => Some(("authorization".into(), format!("Bearer {value}"))),
            Self::Basic => {
                let pair = format!("{}:{value}", username.unwrap_or_default());
                Some((
                    "authorization".into(),
                    format!("Basic {}", encode::base64(pair.as_bytes(), false, true)),
                ))
            }
            Self::Header(name) => Some((name.to_ascii_lowercase(), value.to_owned())),
        }
    }
}

/// Prefix of an agent-secret placeholder: `yulius-secret-<NAME>` stands for NAME's value in a
/// request to one of NAME's hosts (VLT-004).
pub const PLACEHOLDER_PREFIX: &str = "yulius-secret-";

/// The placeholder for `name`.
#[must_use]
pub fn placeholder(name: &SecretName) -> String {
    format!("{PLACEHOLDER_PREFIX}{}", name.as_str())
}

/// The result of replacing placeholders in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Substitution {
    /// The text with every resolvable placeholder replaced.
    pub text: String,
    /// Names whose placeholder was replaced, in order of first use.
    pub used: Vec<String>,
}

/// Replaces each `yulius-secret-NAME` in `text` for which `resolve(NAME)` gives a value;
/// placeholders that do not resolve stay as they are. `None` when nothing was replaced.
pub fn substitute_placeholders(
    text: &str,
    mut resolve: impl FnMut(&str) -> Option<String>,
) -> Option<Substitution> {
    let mut out = String::with_capacity(text.len());
    let mut used: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(PLACEHOLDER_PREFIX) {
        let after = &rest[at + PLACEHOLDER_PREFIX.len()..];
        let name_len = after
            .bytes()
            .take_while(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
            .count();
        let name = &after[..name_len];
        out.push_str(&rest[..at]);
        match (name_len > 0).then(|| resolve(name)).flatten() {
            Some(value) => {
                out.push_str(&value);
                if !used.iter().any(|u| u == name) {
                    used.push(name.to_owned());
                }
            }
            None => {
                out.push_str(PLACEHOLDER_PREFIX);
                out.push_str(name);
            }
        }
        rest = &after[name_len..];
    }
    out.push_str(rest);
    (!used.is_empty()).then_some(Substitution { text: out, used })
}

/// Whether the egress proxy may connect to `ip`: never loopback (Yulius and the sandboxes'
/// own servers), unspecified, link-local (cloud metadata), multicast or broadcast addresses,
/// including their IPv4-mapped IPv6 forms (SPEC-002 ADR-20).
#[must_use]
pub fn egress_destination_allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !(v4.is_loopback()
                || v4.is_unspecified()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast())
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return egress_destination_allowed(IpAddr::V4(mapped));
            }
            let link_local = (v6.segments()[0] & 0xffc0) == 0xfe80;
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || link_local)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reauth_is_fresh_for_ten_minutes() {
        let at = Timestamp(1_000_000);
        assert!(reauth_fresh(at, at));
        assert!(reauth_fresh(at.plus_millis(REAUTH_WINDOW_MS - 1), at));
        assert!(!reauth_fresh(at.plus_millis(REAUTH_WINDOW_MS), at));
        assert!(!reauth_fresh(at.plus_millis(-1), at), "from the future");
    }

    #[test]
    fn agent_values_need_eight_characters_without_controls() {
        assert!(check_agent_value("sk_live_1").is_ok());
        assert!(
            check_agent_value("zażółć12").is_ok(),
            "characters, not bytes"
        );
        assert!(check_agent_value("short12").is_err());
        assert!(check_agent_value("line one\nline two").is_err());
        assert!(check_agent_value(&"x".repeat(MAX_FIELD_BYTES + 1)).is_err());
    }

    /// SPEC-193 MLK-002: a key whose line breaks were lost is refused, saying how to keep them;
    /// the same key with literal `\n` is a value.
    #[test]
    fn a_flattened_key_is_refused() {
        let begin = format!("-----BEGIN {}-----", "SYNTHETIC KEY");
        let end = format!("-----END {}-----", "SYNTHETIC KEY");
        let flat = format!("{begin} c3ludGhldGljLWZsYXQ= {end}");
        let Err(VaultError::Value(message)) = check_agent_value(&flat) else {
            panic!("a flattened key is refused");
        };
        assert!(message.contains("line breaks were lost"), "{message}");
        assert!(message.contains("\\n"), "{message}");
        assert!(check_agent_value(&format!("{begin}\\nc3ludGhldGljLWZsYXQ=\\n{end}")).is_ok());
    }

    #[test]
    fn injections_parse_and_render() -> Result<(), VaultError> {
        assert_eq!(Injection::parse("none", None)?, Injection::None);
        assert_eq!(
            Injection::parse("bearer", None)?.render(None, "tok12345"),
            Some(("authorization".into(), "Bearer tok12345".into()))
        );
        assert_eq!(
            Injection::parse("basic", None)?.render(Some("user"), "pass"),
            Some(("authorization".into(), "Basic dXNlcjpwYXNz".into()))
        );
        assert_eq!(
            Injection::parse("header", Some("X-Api-Key"))?.render(None, "v"),
            Some(("x-api-key".into(), "v".into()))
        );
        assert!(Injection::parse("header", Some("Host")).is_err());
        assert!(Injection::parse("header", Some("Proxy-Authorization")).is_err());
        assert!(Injection::parse("header", Some("a b")).is_err());
        assert!(Injection::parse("cookie", None).is_err());
        Ok(())
    }

    #[test]
    fn placeholders_are_replaced_only_when_resolved() {
        let text = "Bearer yulius-secret-STRIPE_KEY and yulius-secret-OTHER, yulius-secret-";
        let result = substitute_placeholders(text, |name| {
            (name == "STRIPE_KEY").then(|| "sk_live_value".to_owned())
        });
        let Some(result) = result else {
            panic!("expected a substitution");
        };
        assert_eq!(
            result.text,
            "Bearer sk_live_value and yulius-secret-OTHER, yulius-secret-"
        );
        assert_eq!(result.used, vec!["STRIPE_KEY".to_owned()]);
        assert!(substitute_placeholders("nothing here", |_| Some("x".into())).is_none());
        // The longest name wins: API_KEY is not API followed by _KEY.
        let longest =
            substitute_placeholders("yulius-secret-API_KEY", |name| Some(format!("<{name}>")));
        assert_eq!(longest.map(|s| s.text), Some("<API_KEY>".to_owned()));
    }

    #[test]
    fn egress_refuses_local_and_metadata_addresses() -> Result<(), std::net::AddrParseError> {
        for refused in [
            "127.0.0.1",
            "127.8.9.1",
            "0.0.0.0",
            "169.254.169.254",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fe80::1",
            "ff02::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
        ] {
            assert!(!egress_destination_allowed(refused.parse()?), "{refused}");
        }
        for allowed in ["1.1.1.1", "10.0.0.5", "192.168.1.10", "2606:4700::1111"] {
            assert!(egress_destination_allowed(allowed.parse()?), "{allowed}");
        }
        Ok(())
    }
}
