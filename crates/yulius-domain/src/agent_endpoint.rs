//! Where the tools inside a sandbox reach Yulius (SPEC-483 slice 34): `YULIUS_URL`, plain HTTP to
//! a loopback address, and `YULIUS_TASK_TOKEN`, the task's bearer token. The Git credential
//! helper, `yulius wait`, `yulius tool`, the MCP bridge and the hook and redaction clients read
//! them here, one way; each speaks HTTP its own way to the addresses this gives.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// The variable naming Yulius's internal listener.
pub const URL_ENV: &str = "YULIUS_URL";
/// The variable holding the task's bearer token.
pub const TOKEN_ENV: &str = "YULIUS_TASK_TOKEN";

/// Why the environment names no endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EndpointError {
    /// `YULIUS_URL` is not set.
    #[error("YULIUS_URL is not set")]
    UrlMissing,
    /// `YULIUS_TASK_TOKEN` is not set.
    #[error("YULIUS_TASK_TOKEN is not set")]
    TokenMissing,
    /// The token is empty or holds a byte that is not printable ASCII.
    #[error("YULIUS_TASK_TOKEN must be printable ASCII")]
    TokenNotPrintable,
    /// The URL is not `http://host[:port][/path]`.
    #[error("YULIUS_URL must be http:// on a loopback address")]
    Url,
    /// The URL's host is not a loopback address or `localhost`.
    #[error("YULIUS_URL must be http:// on a loopback address")]
    NotLoopback,
}

/// Yulius's internal listener as a tool in the sandbox reaches it.
#[derive(Clone, PartialEq, Eq)]
pub struct AgentEndpoint {
    /// The addresses to try, in order: the literal address, or for `localhost` 127.0.0.1 then
    /// ::1 (no name lookup).
    pub addresses: Vec<SocketAddr>,
    /// The `Host` header: the URL's authority as written.
    pub host_header: String,
    /// The URL's path without a trailing `/`, prefixed to every request path.
    pub base_path: String,
    /// The task's bearer token.
    pub token: String,
}

impl fmt::Debug for AgentEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentEndpoint")
            .field("addresses", &self.addresses)
            .field("host_header", &self.host_header)
            .field("base_path", &self.base_path)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl AgentEndpoint {
    /// The endpoint the environment `env` names.
    ///
    /// # Errors
    /// [`EndpointError`] for a missing variable, a token that is not printable ASCII, or a URL
    /// that is not plain HTTP to a loopback address.
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Result<Self, EndpointError> {
        let url = env(URL_ENV).ok_or(EndpointError::UrlMissing)?;
        let token = env(TOKEN_ENV).ok_or(EndpointError::TokenMissing)?;
        if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(EndpointError::TokenNotPrintable);
        }
        let (addresses, host_header, base_path) = parse_url(&url)?;
        Ok(Self {
            addresses,
            host_header,
            base_path,
            token,
        })
    }
}

/// Parses `http://host[:port][/path]`: printable ASCII only, no user information, query or
/// fragment, a literal loopback address (bracketed for IPv6) or `localhost`, port 80 when absent.
fn parse_url(url: &str) -> Result<(Vec<SocketAddr>, String, String), EndpointError> {
    let rest = url.strip_prefix("http://").ok_or(EndpointError::Url)?;
    if !rest.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(EndpointError::Url);
    }
    let (authority, path) = match rest.find('/') {
        Some(slash) => (&rest[..slash], &rest[slash..]),
        None => (rest, ""),
    };
    if authority.is_empty() || authority.contains('@') || path.contains(['?', '#']) {
        return Err(EndpointError::Url);
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']').ok_or(EndpointError::Url)?;
        let port = match after {
            "" => "80",
            port => port.strip_prefix(':').ok_or(EndpointError::Url)?,
        };
        (host, port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, port),
            None => (authority, "80"),
        }
    };
    let port: u16 = port.parse().map_err(|_| EndpointError::Url)?;
    let ips = if host.eq_ignore_ascii_case("localhost") {
        vec![
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ]
    } else {
        let ip = host
            .parse::<IpAddr>()
            .map_err(|_| EndpointError::NotLoopback)?;
        if !ip.is_loopback() {
            return Err(EndpointError::NotLoopback);
        }
        vec![ip]
    };
    Ok((
        ips.into_iter()
            .map(|ip| SocketAddr::new(ip, port))
            .collect(),
        authority.to_owned(),
        path.trim_end_matches('/').to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(url: &str, token: &str) -> Result<AgentEndpoint, EndpointError> {
        let (url, token) = (url.to_owned(), token.to_owned());
        AgentEndpoint::from_env(&move |name| match name {
            URL_ENV => Some(url.clone()),
            TOKEN_ENV => Some(token.clone()),
            _ => None,
        })
    }

    /// SPEC-483 slice 34 (QB2): one reader of the endpoint for every client in the sandbox, with
    /// the cases the Git client's and the hook client's parsers each had.
    #[test]
    fn the_endpoint_is_plain_http_to_loopback_with_a_printable_token() -> Result<(), EndpointError>
    {
        let based = with("http://127.0.0.1:7171/base/", "ytk_a")?;
        assert_eq!(
            based.addresses,
            vec![SocketAddr::from(([127, 0, 0, 1], 7171))]
        );
        assert_eq!(
            (based.host_header.as_str(), based.base_path.as_str()),
            ("127.0.0.1:7171", "/base")
        );
        let v6 = with("http://[::1]:7", "ytk_a")?;
        assert_eq!(
            v6.addresses,
            vec![SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 7)]
        );
        let local = with("http://LocalHost:9/internal/", "ytk_a")?;
        assert_eq!(local.addresses.len(), 2);
        assert!(
            local
                .addresses
                .iter()
                .all(|a| a.ip().is_loopback() && a.port() == 9)
        );
        assert_eq!(local.base_path, "/internal");
        assert_eq!(with("http://127.0.0.2", "ytk_a")?.addresses[0].port(), 80);
        for (url, why) in [
            ("https://127.0.0.1:1", EndpointError::Url),
            ("http://", EndpointError::Url),
            ("http://user@127.0.0.1:80", EndpointError::Url),
            ("http://127.0.0.1:99999", EndpointError::Url),
            ("http://127.0.0.1:x", EndpointError::Url),
            ("http://[::1]x", EndpointError::Url),
            ("http://127.0.0.1/a b", EndpointError::Url),
            ("http://127.0.0.1/a?b", EndpointError::Url),
            ("http://127.0.0.1/\u{e9}", EndpointError::Url),
            ("http://10.0.0.1:80", EndpointError::NotLoopback),
            ("http://0.0.0.0:80", EndpointError::NotLoopback),
            ("http://example.com", EndpointError::NotLoopback),
        ] {
            assert_eq!(with(url, "ytk_a").err(), Some(why), "{url}");
        }
        for token in ["", "a b", "\u{e9}t", "a\n"] {
            assert_eq!(
                with("http://127.0.0.1:1", token).err(),
                Some(EndpointError::TokenNotPrintable),
                "{token:?}"
            );
        }
        let missing = AgentEndpoint::from_env(&|_| None).err();
        assert_eq!(missing, Some(EndpointError::UrlMissing));
        let url_only = AgentEndpoint::from_env(&|name| {
            (name == URL_ENV).then(|| "http://127.0.0.1".to_owned())
        });
        assert_eq!(url_only.err(), Some(EndpointError::TokenMissing));
        assert!(!format!("{based:?}").contains("ytk_a"));
        Ok(())
    }
}
