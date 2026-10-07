//! Minimal HTTP/1.1 health probe (no HTTP client dependency for one GET on loopback).

use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

/// Largest response head read.
const MAX_HEAD_BYTES: usize = 4096;

/// Sends `GET <path>` to `127.0.0.1:<port>` with an optional `Cookie` header and returns the
/// status code, or a short description of why no status was received within `timeout`.
pub(crate) async fn probe(
    port: u16,
    path: &str,
    cookie: Option<&str>,
    timeout: Duration,
) -> Result<u16, String> {
    let exchange = async {
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .map_err(|error| format!("connect: {error}"))?;
        let cookie_line = cookie
            .map(|cookie| format!("Cookie: {cookie}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{cookie_line}Connection: close\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|error| format!("write: {error}"))?;
        let mut head = Vec::with_capacity(512);
        let mut buf = [0_u8; 512];
        while !head.windows(2).any(|w| w == b"\r\n") && head.len() < MAX_HEAD_BYTES {
            let n = stream
                .read(&mut buf)
                .await
                .map_err(|error| format!("read: {error}"))?;
            if n == 0 {
                break;
            }
            head.extend_from_slice(&buf[..n]);
        }
        parse_status(&head).ok_or_else(|| "no HTTP status line".to_owned())
    };
    tokio::time::timeout(timeout, exchange)
        .await
        .unwrap_or_else(|_| Err(format!("no answer within {timeout:?}")))
}

fn parse_status(head: &[u8]) -> Option<u16> {
    let line = head.split(|&b| b == b'\n').next()?;
    let line = std::str::from_utf8(line).ok()?;
    let mut parts = line.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/1.") {
        return None;
    }
    parts.next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_lines() {
        assert_eq!(parse_status(b"HTTP/1.1 200 OK\r\nX: y\r\n"), Some(200));
        assert_eq!(parse_status(b"HTTP/1.0 403 Forbidden\r\n"), Some(403));
        assert_eq!(parse_status(b"SSH-2.0-OpenSSH\r\n"), None);
        assert_eq!(parse_status(b""), None);
    }

    #[tokio::test]
    async fn probe_reports_refused_connections() -> Result<(), Box<dyn std::error::Error>> {
        let port = std::net::TcpListener::bind("127.0.0.1:0")?
            .local_addr()?
            .port();
        let outcome = probe(port, "/", None, Duration::from_secs(2)).await;
        assert!(
            matches!(outcome, Err(ref text) if text.starts_with("connect")),
            "{outcome:?}"
        );
        Ok(())
    }
}
