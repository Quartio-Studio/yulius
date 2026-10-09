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
    /// The address is Yulius itself (its public origin, a preview origin or one of its
    /// listeners): opening it inside its own browser would recurse (BRW-008).
    #[error("Yulius itself cannot be opened in the sandbox browser")]
    Yulius,
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

/// Addresses the sandbox browser never loads: Yulius's own origins and listeners (BRW-008).
///
/// Patterns use the Chrome DevTools `Fetch` wildcard syntax (`*` any run of characters, `?` one
/// character) and are matched against the whole URL; the same patterns are handed to Chromium
/// so that page-initiated loads (links, redirects, scripts, frames) are intercepted too.
/// `allow` holds URL prefixes that stay reachable although a pattern matches them (the welcome
/// page on the internal listener).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UrlDenyList {
    patterns: Vec<String>,
    allow: Vec<String>,
}

impl UrlDenyList {
    /// A list from explicit patterns and allowed prefixes.
    #[must_use]
    pub fn new(patterns: Vec<String>, allow: Vec<String>) -> Self {
        Self { patterns, allow }
    }

    /// The denials for a Yulius instance: every loopback form of its listener addresses
    /// (`listeners`), its public origin and, when the public host is a domain name, every
    /// subdomain of it (the preview origins). `allow` keeps prefixes such as the welcome page.
    #[must_use]
    pub fn yulius(
        public_url: &str,
        listeners: &[std::net::SocketAddr],
        allow: Vec<String>,
    ) -> Self {
        let mut patterns = Vec::new();
        for listener in listeners {
            let port = listener.port();
            if listener.ip().is_unspecified() {
                patterns.push(format!("*://*:{port}/*"));
                continue;
            }
            let ip = listener.ip();
            let host = if ip.is_ipv6() {
                format!("[{ip}]")
            } else {
                ip.to_string()
            };
            patterns.push(format!("*://{host}:{port}/*"));
            if ip.is_loopback() {
                for alias in ["127.0.0.1", "localhost", "*.localhost", "[::1]", "0.0.0.0"] {
                    patterns.push(format!("*://{alias}:{port}/*"));
                }
            }
        }
        if let Some((scheme, rest)) = public_url.split_once("://") {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
            let authority = authority.to_ascii_lowercase();
            let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
                let (inner, after) = rest.split_once(']').unwrap_or((rest, ""));
                (
                    format!("[{inner}]"),
                    after.strip_prefix(':').map(str::to_owned),
                )
            } else {
                match authority.rsplit_once(':') {
                    Some((host, port)) => (host.to_owned(), Some(port.to_owned())),
                    None => (authority.clone(), None),
                }
            };
            let bare = host.trim_start_matches('[').trim_end_matches(']');
            let is_ip = bare.parse::<std::net::IpAddr>().is_ok();
            let loopback = is_loopback(&host);
            if !host.is_empty() && !loopback {
                if is_ip {
                    // Only the exact origin: other ports of that address are not Yulius.
                    let default = if scheme.eq_ignore_ascii_case("https") {
                        "443"
                    } else {
                        "80"
                    };
                    match port.as_deref() {
                        Some(port) if port != default => {
                            patterns.push(format!("*://{host}:{port}/*"))
                        }
                        _ => patterns.push(format!("*://{host}/*")),
                    }
                } else {
                    for name in [host.clone(), format!("*.{host}")] {
                        patterns.push(format!("*://{name}/*"));
                        patterns.push(format!("*://{name}:*/*"));
                    }
                }
            }
        }
        patterns.sort();
        patterns.dedup();
        Self { patterns, allow }
    }

    /// The patterns (for `Fetch.enable`).
    #[must_use]
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// Whether the list denies nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether `url` must not be loaded.
    #[must_use]
    pub fn denies(&self, url: &str) -> bool {
        let lowered = url.to_ascii_lowercase();
        self.patterns
            .iter()
            .any(|pattern| wildcard_match(pattern.as_bytes(), lowered.as_bytes()))
            && !self
                .allow
                .iter()
                .any(|prefix| url.starts_with(prefix.as_str()))
    }
}

/// Chrome's `Fetch` URL pattern matching: `*` matches any run (also empty), `?` one byte,
/// `\` escapes the next byte.
fn wildcard_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(b'*') => {
                star = Some((p, t));
                p += 1;
            }
            Some(b'?') => {
                p += 1;
                t += 1;
            }
            Some(b'\\') if pattern.get(p + 1) == text.get(t) && p + 1 < pattern.len() => {
                p += 2;
                t += 1;
            }
            Some(byte) if *byte != b'\\' && *byte == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some((star_p, star_t)) => {
                    p = star_p + 1;
                    t = star_t + 1;
                    star = Some((star_p, star_t + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|b| *b == b'*')
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
    fn wildcards_match_like_chrome() {
        assert!(wildcard_match(
            b"*://localhost:8080/*",
            b"http://localhost:8080/"
        ));
        assert!(wildcard_match(
            b"*://localhost:8080/*",
            b"ws://localhost:8080/api?x"
        ));
        assert!(!wildcard_match(
            b"*://localhost:8080/*",
            b"http://localhost:80801/"
        ));
        assert!(!wildcard_match(
            b"*://localhost:8080/*",
            b"http://localhost:8081/"
        ));
        assert!(wildcard_match(b"a?c", b"abc"));
        assert!(!wildcard_match(b"a?c", b"ac"));
        assert!(wildcard_match(b"*", b""));
        assert!(wildcard_match(b"a\\*", b"a*"));
        assert!(!wildcard_match(b"a\\*", b"ab"));
    }

    #[test]
    fn the_deny_list_covers_yulius_origins_and_listeners_only() {
        let listeners: Vec<std::net::SocketAddr> = ["127.0.0.1:8080", "127.0.0.1:8081"]
            .iter()
            .filter_map(|a| a.parse().ok())
            .collect();
        let deny = UrlDenyList::yulius(
            "https://109-199-123-240.sslip.io",
            &listeners,
            vec![
                "http://127.0.0.1:8081/browser/welcome?".into(),
                "http://127.0.0.1:8081/browser/welcome/ports?".into(),
            ],
        );
        for url in [
            "http://127.0.0.1:8080/",
            "http://localhost:8080/s/x",
            "http://LOCALHOST:8081/mcp",
            "http://app.localhost:8080/",
            "http://[::1]:8080/",
            "http://0.0.0.0:8081/hooks",
            "https://109-199-123-240.sslip.io/",
            "https://109-199-123-240.sslip.io:8443/api/me",
            "https://20001-0123456789ab.109-199-123-240.sslip.io/",
            "http://127.0.0.1:8081/browser/welcomeX",
        ] {
            assert!(deny.denies(url), "{url}");
        }
        for url in [
            "http://localhost:20000/",
            "http://127.0.0.1:3000/",
            "https://example.com/",
            "https://sslip.io/",
            "https://other-109-199-123-240.sslip.io.example.com/",
            "http://127.0.0.1:8081/browser/welcome?sandbox=s1",
            "http://127.0.0.1:8081/browser/welcome/ports?sandbox=s1",
        ] {
            assert!(!deny.denies(url), "{url}");
        }
        // A development instance on loopback: only its ports, never all of 127.0.0.1.
        let dev = UrlDenyList::yulius("http://127.0.0.1:8080", &listeners, Vec::new());
        assert!(dev.denies("http://127.0.0.1:8080/"));
        assert!(!dev.denies("http://127.0.0.1:20000/"));
        // A server bound to every address: the port on any host.
        let any: Vec<std::net::SocketAddr> = ["0.0.0.0:8080"]
            .iter()
            .filter_map(|a| a.parse().ok())
            .collect();
        let wide = UrlDenyList::yulius("http://10.0.0.5:8080", &any, Vec::new());
        assert!(wide.denies("http://10.0.0.5:8080/"));
        assert!(wide.denies("http://192.168.1.2:8080/"));
        assert!(!wide.denies("http://10.0.0.5:20000/"));
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
