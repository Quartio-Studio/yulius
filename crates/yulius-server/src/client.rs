//! The client of a public request, for per-client limits (SPEC-001 ACC-013, SPEC-002 §8).
//!
//! Behind the TLS reverse proxy every TCP peer is the proxy, so a limit keyed on the peer is one
//! bucket for the whole internet. The proxy therefore passes the client's address in
//! [`CLIENT_ADDRESS_HEADER`] together with a secret shared only by the proxy and Yulius in
//! [`PROXY_SECRET_HEADER`], overwriting whatever a client sent in either. Yulius trusts the
//! address only when the request carries exactly one secret header equal (in constant time) to
//! the configured secret and exactly one well-formed address. Anything else (no secret
//! configured, a missing, repeated or wrong secret, a missing or malformed address) keys on the
//! TCP peer, as before, so a process that reaches the public listener directly cannot pick its
//! bucket by forging headers. Neither header's value is ever logged or echoed.

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::HeaderMap;
use axum::http::request::Parts;
use subtle::ConstantTimeEq as _;
use yulius_app::error::AppError;
use yulius_app::model::Secret;

use crate::AppState;
use crate::error::ApiError;

/// Request header carrying the client address the trusted reverse proxy saw (an IP address).
pub const CLIENT_ADDRESS_HEADER: &str = "x-yulius-client-ip";
/// Request header carrying the secret shared by the reverse proxy and Yulius.
pub const PROXY_SECRET_HEADER: &str = "x-yulius-proxy-secret";

/// Whether the one-time warning about an untrusted client address was logged.
static UNTRUSTED_WARNED: AtomicBool = AtomicBool::new(false);

/// The only value of `name`, or `None` when it is absent or repeated.
fn single<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a [u8]> {
    let mut values = headers.get_all(name).iter();
    let first = values.next()?;
    values.next().is_none().then(|| first.as_bytes())
}

/// Parses an address as proxies write it: `203.0.113.7`, `2001:db8::1` or `[2001:db8::1]`.
fn parse_address(raw: &[u8]) -> Option<IpAddr> {
    let text = std::str::from_utf8(raw).ok()?.trim();
    let text = text
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(text);
    text.parse().ok()
}

/// Whether the request carries exactly one secret header equal to `secret` (constant time).
fn carries_secret(secret: &Secret, headers: &HeaderMap) -> bool {
    single(headers, PROXY_SECRET_HEADER)
        .is_some_and(|sent| bool::from(sent.ct_eq(secret.expose().as_bytes())))
}

/// The client address of a request from `peer`: the trusted proxy's address when the request
/// carries `proxy_secret` (ACC-013), else the peer.
#[must_use]
pub fn client_address(proxy_secret: Option<&Secret>, headers: &HeaderMap, peer: IpAddr) -> IpAddr {
    let trusted = proxy_secret
        .filter(|secret| carries_secret(secret, headers))
        .and_then(|_| single(headers, CLIENT_ADDRESS_HEADER).and_then(parse_address));
    if let Some(address) = trusted {
        return address;
    }
    if headers.contains_key(CLIENT_ADDRESS_HEADER)
        && !UNTRUSTED_WARNED.swap(true, Ordering::Relaxed)
    {
        // Once per process, and without either header's value.
        tracing::warn!(
            "a request carried {CLIENT_ADDRESS_HEADER} that is not trusted (no proxy secret is \
             configured, or the request's secret or address is missing, repeated or wrong); \
             per-client limits key such requests on the TCP peer (docs/operations.md §4)"
        );
    }
    peer
}

/// The key of a client in per-client limits: an IPv4 address (also when written as IPv4-mapped
/// IPv6), or the /64 network of an IPv6 address, which one subscriber usually holds whole.
#[must_use]
pub fn limit_key(address: IpAddr) -> String {
    match address {
        IpAddr::V4(v4) => v4.to_string(),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None => {
                let network = u128::from(v6) & (u128::MAX << 64);
                format!("{}/64", Ipv6Addr::from(network))
            }
        },
    }
}

/// The client of a public request (see the module documentation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAddress(pub IpAddr);

impl ClientAddress {
    /// This client's key in per-client limits ([`limit_key`]).
    #[must_use]
    pub fn limit_key(self) -> String {
        limit_key(self.0)
    }
}

impl FromRequestParts<AppState> for ClientAddress {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        app: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // The extractor also takes a test's `MockConnectInfo`.
        let ConnectInfo(peer) = ConnectInfo::<SocketAddr>::from_request_parts(parts, app)
            .await
            .map_err(|_| AppError::internal("the listener does not record peer addresses"))?;
        Ok(Self(client_address(
            app.config.proxy_secret.as_ref(),
            &parts.headers,
            peer.ip(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const SECRET: &str = "4f1c0d9e8b7a6f5e4d3c2b1a09f8e7d6c5b4a3928170f6e5d4c3b2a1908f7e6d";

    fn secret() -> Secret {
        Secret(SECRET.into())
    }

    fn peer() -> IpAddr {
        IpAddr::from([127, 0, 0, 1])
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            if let Ok(value) = HeaderValue::from_str(value) {
                map.append(*name, value);
            }
        }
        map
    }

    #[test]
    fn the_proxys_address_counts_only_with_the_matching_secret() {
        let proxied = headers(&[
            (CLIENT_ADDRESS_HEADER, "203.0.113.7"),
            (PROXY_SECRET_HEADER, SECRET),
        ]);
        assert_eq!(
            client_address(Some(&secret()), &proxied, peer()),
            IpAddr::from([203, 0, 113, 7])
        );
        // No secret configured: unchanged behaviour, the peer.
        assert_eq!(client_address(None, &proxied, peer()), peer());
        let wrong = format!("{}x", &SECRET[..SECRET.len() - 1]);
        let prefix = &SECRET[..32];
        let longer = format!("{SECRET}0");
        for refused in [
            headers(&[(CLIENT_ADDRESS_HEADER, "203.0.113.7")]),
            headers(&[
                (CLIENT_ADDRESS_HEADER, "203.0.113.7"),
                (PROXY_SECRET_HEADER, &wrong),
            ]),
            headers(&[
                (CLIENT_ADDRESS_HEADER, "203.0.113.7"),
                (PROXY_SECRET_HEADER, prefix),
            ]),
            headers(&[
                (CLIENT_ADDRESS_HEADER, "203.0.113.7"),
                (PROXY_SECRET_HEADER, &longer),
            ]),
            headers(&[
                (CLIENT_ADDRESS_HEADER, "203.0.113.7"),
                (PROXY_SECRET_HEADER, ""),
            ]),
            // A client's own copy next to the proxy's: repeated, so not trusted.
            headers(&[
                (CLIENT_ADDRESS_HEADER, "203.0.113.7"),
                (PROXY_SECRET_HEADER, SECRET),
                (PROXY_SECRET_HEADER, SECRET),
            ]),
            headers(&[
                (CLIENT_ADDRESS_HEADER, "203.0.113.7"),
                (CLIENT_ADDRESS_HEADER, "198.51.100.1"),
                (PROXY_SECRET_HEADER, SECRET),
            ]),
            // The right secret without a usable address.
            headers(&[(PROXY_SECRET_HEADER, SECRET)]),
            headers(&[
                (CLIENT_ADDRESS_HEADER, "not-an-ip"),
                (PROXY_SECRET_HEADER, SECRET),
            ]),
            headers(&[
                (CLIENT_ADDRESS_HEADER, "203.0.113.7, 198.51.100.1"),
                (PROXY_SECRET_HEADER, SECRET),
            ]),
            headers(&[
                (CLIENT_ADDRESS_HEADER, "203.0.113.7:443"),
                (PROXY_SECRET_HEADER, SECRET),
            ]),
        ] {
            assert_eq!(client_address(Some(&secret()), &refused, peer()), peer());
        }
    }

    #[test]
    fn addresses_parse_as_proxies_write_them() {
        let with = |address: &str| {
            client_address(
                Some(&secret()),
                &headers(&[
                    (CLIENT_ADDRESS_HEADER, address),
                    (PROXY_SECRET_HEADER, SECRET),
                ]),
                peer(),
            )
        };
        let v6: IpAddr = "2001:db8::1".parse().unwrap_or(peer());
        assert_eq!(with("2001:db8::1"), v6);
        assert_eq!(with("[2001:db8::1]"), v6);
        assert_eq!(with(" 198.51.100.4 "), IpAddr::from([198, 51, 100, 4]));
    }

    #[test]
    fn limit_keys_group_an_ipv6_subscriber_and_unmap_ipv4() {
        let key = |text: &str| text.parse::<IpAddr>().map(limit_key).unwrap_or_default();
        assert_eq!(key("203.0.113.7"), "203.0.113.7");
        assert_eq!(key("::ffff:203.0.113.7"), "203.0.113.7");
        assert_eq!(key("2001:db8:1:2:aaaa::1"), "2001:db8:1:2::/64");
        assert_eq!(key("2001:db8:1:2:ffff:ffff:ffff:ffff"), "2001:db8:1:2::/64");
        assert_ne!(key("2001:db8:1:3::1"), key("2001:db8:1:2::1"));
        assert_eq!(key("::1"), "::/64");
    }
}
