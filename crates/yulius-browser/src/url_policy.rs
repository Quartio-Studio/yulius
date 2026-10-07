//! Address policy for viewer and tool navigation (SPEC-001 BRW-001; WP-E URL policy).
//!
//! Only `http`, `https` and the exact address `about:blank` are allowed. Everything else
//! (`file:`, `chrome:`, `javascript:`, `data:`, `view-source:`, `ftp:`, …) is refused with a
//! typed error before any DevTools call. A bare host such as `example.com/x` or
//! `localhost:3000` gets `https://` (or `http://` for loopback hosts) so typed addresses work
//! like a normal address bar.

use yulius_contracts::browser::URL_MAX_BYTES;

/// Why an address was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlRefused {
    /// Empty or whitespace-only input.
    #[error("address is empty")]
    Empty,
    /// Longer than the bound.
    #[error("address exceeds {0} bytes")]
    TooLong(usize),
    /// Whitespace or control characters inside the address.
    #[error("address contains whitespace or control characters")]
    InvalidCharacters,
    /// A scheme other than http/https (or an `about:` page other than `about:blank`).
    #[error("scheme {0:?} is not allowed; only http, https and about:blank")]
    Scheme(String),
    /// `scheme://` without a host.
    #[error("address has no host")]
    MissingHost,
}

/// Schemes named explicitly for clearer refusals; any other non-http(s) scheme is refused too.
const BLOCKED_SCHEMES: &[&str] = &[
    "about",
    "blob",
    "chrome",
    "chrome-extension",
    "chrome-untrusted",
    "data",
    "devtools",
    "file",
    "filesystem",
    "javascript",
    "view-source",
    "vbscript",
];

/// Validates an address and returns the URL to load.
///
/// # Errors
/// [`UrlRefused`] for empty, oversized or malformed input and for any scheme other than
/// `http`/`https` (plus exact `about:blank`).
pub fn normalize_url(input: &str) -> Result<String, UrlRefused> {
    let text = input.trim();
    if text.is_empty() {
        return Err(UrlRefused::Empty);
    }
    if text.len() > URL_MAX_BYTES {
        return Err(UrlRefused::TooLong(URL_MAX_BYTES));
    }
    if text.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(UrlRefused::InvalidCharacters);
    }
    if text.eq_ignore_ascii_case("about:blank") {
        return Ok("about:blank".to_owned());
    }
    if let Some((scheme, rest)) = split_scheme(text) {
        let scheme = scheme.to_ascii_lowercase();
        if let Some(after_slashes) = rest.strip_prefix("//") {
            if scheme == "http" || scheme == "https" {
                check_host(after_slashes)?;
                return Ok(format!("{scheme}:{rest}"));
            }
            return Err(UrlRefused::Scheme(scheme));
        }
        let looks_like_port = rest.bytes().next().is_some_and(|b| b.is_ascii_digit());
        if BLOCKED_SCHEMES.contains(&scheme.as_str()) || !looks_like_port {
            return Err(UrlRefused::Scheme(scheme));
        }
    }
    let host = host_of(text);
    let prefix = if is_loopback(host) {
        "http://"
    } else {
        "https://"
    };
    check_host(text)?;
    Ok(format!("{prefix}{text}"))
}

/// Splits `scheme:rest` when the prefix matches RFC 3986 scheme syntax.
fn split_scheme(text: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = text.split_once(':')?;
    let mut bytes = scheme.bytes();
    let first_ok = bytes.next().is_some_and(|b| b.is_ascii_alphabetic());
    let rest_ok = bytes.all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.');
    (first_ok && rest_ok).then_some((scheme, rest))
}

/// The authority's host (no userinfo, no port) of `host[:port][/path]`.
fn host_of(authority_and_path: &str) -> &str {
    let authority = authority_and_path
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    if host_port.starts_with('[') {
        return host_port
            .split_once(']')
            .map_or(host_port, |(host, _)| host.trim_start_matches('['));
    }
    host_port.split(':').next().unwrap_or_default()
}

fn check_host(authority_and_path: &str) -> Result<(), UrlRefused> {
    if host_of(authority_and_path).is_empty() {
        Err(UrlRefused::MissingHost)
    } else {
        Ok(())
    }
}

fn is_loopback(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host.ends_with(".localhost")
        || host.starts_with("127.")
        || host == "::1"
        || host == "0.0.0.0"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_http_https_and_about_blank() {
        assert_eq!(
            normalize_url("https://example.com/a?b#c"),
            Ok("https://example.com/a?b#c".into())
        );
        assert_eq!(
            normalize_url("HTTP://Example.com"),
            Ok("http://Example.com".into())
        );
        assert_eq!(normalize_url("  about:blank "), Ok("about:blank".into()));
        assert_eq!(normalize_url("About:Blank"), Ok("about:blank".into()));
        assert_eq!(
            normalize_url("http://user:pw@[::1]:8080/"),
            Ok("http://user:pw@[::1]:8080/".into())
        );
    }

    #[test]
    fn bare_hosts_get_a_scheme() {
        assert_eq!(
            normalize_url("example.com/x"),
            Ok("https://example.com/x".into())
        );
        assert_eq!(
            normalize_url("localhost:3000/app"),
            Ok("http://localhost:3000/app".into())
        );
        assert_eq!(
            normalize_url("127.0.0.1:20001"),
            Ok("http://127.0.0.1:20001".into())
        );
        assert_eq!(
            normalize_url("example.com:8443"),
            Ok("https://example.com:8443".into())
        );
    }

    #[test]
    fn refuses_dangerous_and_unknown_schemes() {
        for (input, scheme) in [
            ("file:///etc/passwd", "file"),
            ("FILE:///etc/passwd", "file"),
            ("file:/etc/passwd", "file"),
            ("chrome://settings", "chrome"),
            ("chrome:settings", "chrome"),
            ("javascript:alert(1)", "javascript"),
            ("JavaScript:alert(1)", "javascript"),
            ("data:text/html,hi", "data"),
            ("view-source:https://example.com", "view-source"),
            ("about:config", "about"),
            ("ftp://example.com", "ftp"),
            ("chrome-extension://abc/x", "chrome-extension"),
            ("mailto:a@example.com", "mailto"),
            ("blob:https://x/1", "blob"),
        ] {
            assert_eq!(
                normalize_url(input),
                Err(UrlRefused::Scheme(scheme.into())),
                "{input}"
            );
        }
    }

    #[test]
    fn refuses_malformed_input() {
        assert_eq!(normalize_url("   "), Err(UrlRefused::Empty));
        assert_eq!(
            normalize_url("https://exa mple.com"),
            Err(UrlRefused::InvalidCharacters)
        );
        assert_eq!(
            normalize_url("https://x\u{0}"),
            Err(UrlRefused::InvalidCharacters)
        );
        assert_eq!(normalize_url("https://"), Err(UrlRefused::MissingHost));
        assert_eq!(normalize_url("http:///path"), Err(UrlRefused::MissingHost));
        let long = format!("https://example.com/{}", "a".repeat(URL_MAX_BYTES));
        assert_eq!(
            normalize_url(&long),
            Err(UrlRefused::TooLong(URL_MAX_BYTES))
        );
    }
}
