//! Client side of the `yulius git-credential` helper (SPEC-002 §8, R-06).
//!
//! Git runs the helper as `yulius git-credential <get|store|erase>` with the credential
//! description on stdin. For `get`, the helper asks the Yulius internal listener
//! (`${YULIUS_URL}`, loopback only) with the attempt's bearer token
//! (`${YULIUS_TASK_TOKEN}`) and prints `username=x-access-token` / `password=<token>`.
//! `store` and `erase` are no-ops because Yulius owns token lifetime.
//!
//! Wire contract with the server (integrator implements the route):
//!
//! ```text
//! POST {YULIUS_URL}/internal/git-credential HTTP/1.1
//! Authorization: Bearer <task token>
//! Content-Type: application/json
//!
//! {"protocol":"https","host":"github.com","path":"owner/repo.git"}   (path only if sent by git)
//!
//! 200 {"token":"ghs_…","expires_at":1767225600}   (expires_at: optional Unix seconds)
//! 401/403/404 → no credential
//! ```

use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// Overall deadline for one `get` round trip.
pub const HELPER_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound on the credential description read from Git.
const MAX_INPUT_BYTES: usize = 64 * 1024;
/// Bound on the HTTP response read from the server.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// Username GitHub expects alongside an installation or user access token.
pub const TOKEN_USERNAME: &str = "x-access-token";
/// Internal route the helper calls.
pub const CREDENTIAL_PATH: &str = "/internal/git-credential";

/// Exit code: credential printed, or no-op operation.
pub const EXIT_OK: i32 = 0;
/// Exit code: the server refused, was unreachable or answered badly.
pub const EXIT_UNAVAILABLE: i32 = 1;
/// Exit code: bad input or configuration (missing/invalid environment).
pub const EXIT_USAGE: i32 = 2;

/// The credential description Git sends (only the fields Yulius uses).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CredentialRequest {
    /// `protocol=` (for example `https`).
    pub protocol: Option<String>,
    /// `host=` (may include a port).
    pub host: Option<String>,
    /// `path=` (sent when `credential.useHttpPath` is enabled).
    pub path: Option<String>,
}

/// A credential returned by the server. `Debug` hides the token.
#[derive(Clone, PartialEq, Eq)]
pub struct Credential {
    /// Access token used as the password.
    pub token: String,
    /// Optional expiry in Unix seconds (printed as `password_expiry_utc`).
    pub expires_at: Option<i64>,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credential")
            .field("token", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Why a credential could not be obtained. Messages never include the task token or the
/// returned access token.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CredentialError {
    /// The credential description from Git was malformed or too large.
    #[error("malformed credential input: {0}")]
    BadInput(&'static str),
    /// A required environment variable was missing or invalid.
    #[error("invalid helper environment: {0}")]
    BadEnvironment(&'static str),
    /// Only `https` credentials are served.
    #[error("credentials are only served for https")]
    UnsupportedProtocol,
    /// The server declined (401/403/404).
    #[error("Yulius refused the credential request (HTTP {0})")]
    Refused(u16),
    /// The server answered with an unexpected status.
    #[error("Yulius answered HTTP {0}")]
    Upstream(u16),
    /// Connecting, writing or reading failed or timed out.
    #[error("could not reach Yulius: {0}")]
    Transport(String),
    /// The response could not be parsed.
    #[error("bad response from Yulius: {0}")]
    BadResponse(&'static str),
}

impl CredentialError {
    fn exit_code(&self) -> i32 {
        match self {
            Self::BadInput(_) | Self::BadEnvironment(_) => EXIT_USAGE,
            _ => EXIT_UNAVAILABLE,
        }
    }
}

/// Runs one helper operation speaking Git's credential-helper protocol and returns the
/// process exit code ([`EXIT_OK`], [`EXIT_UNAVAILABLE`], [`EXIT_USAGE`]).
///
/// `env` looks up `YULIUS_URL` (must be `http://` on a loopback address) and
/// `YULIUS_TASK_TOKEN`. Unknown operations are ignored, as the protocol requires.
pub fn run_credential_helper(
    op: &str,
    input: &mut dyn Read,
    output: &mut dyn Write,
    env: &dyn Fn(&str) -> Option<String>,
) -> i32 {
    let request = match read_request(input) {
        Ok(request) => request,
        Err(error) => return error.exit_code(),
    };
    if op != "get" {
        return EXIT_OK;
    }
    match credential_get(&request, env, HELPER_TIMEOUT) {
        Ok(credential) => match write_credential(output, &credential) {
            Ok(()) => EXIT_OK,
            Err(_) => EXIT_UNAVAILABLE,
        },
        Err(error) => error.exit_code(),
    }
}

/// Parses Git's `key=value` credential description (until a blank line or EOF).
pub fn read_request(input: &mut dyn Read) -> Result<CredentialRequest, CredentialError> {
    let limited = input.take(MAX_INPUT_BYTES as u64 + 1);
    let mut reader = BufReader::new(limited);
    let mut request = CredentialRequest::default();
    let mut total = 0_usize;
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|_| CredentialError::BadInput("unreadable or non-UTF-8 input"))?;
        if read == 0 {
            break;
        }
        total += read;
        if total > MAX_INPUT_BYTES {
            return Err(CredentialError::BadInput("input too large"));
        }
        let entry = line.trim_end_matches(['\n', '\r']);
        if entry.is_empty() {
            break;
        }
        let Some((key, value)) = entry.split_once('=') else {
            return Err(CredentialError::BadInput("line without '='"));
        };
        match key {
            "protocol" => request.protocol = Some(value.to_owned()),
            "host" => request.host = Some(value.to_owned()),
            "path" => request.path = Some(value.to_owned()),
            _ => {}
        }
    }
    Ok(request)
}

/// Asks the Yulius internal listener for a credential, within `timeout` overall.
pub fn credential_get(
    request: &CredentialRequest,
    env: &dyn Fn(&str) -> Option<String>,
    timeout: Duration,
) -> Result<Credential, CredentialError> {
    if request.protocol.as_deref() != Some("https") {
        return Err(CredentialError::UnsupportedProtocol);
    }
    let base = env("YULIUS_URL").ok_or(CredentialError::BadEnvironment("YULIUS_URL is not set"))?;
    let token = env("YULIUS_TASK_TOKEN").ok_or(CredentialError::BadEnvironment(
        "YULIUS_TASK_TOKEN is not set",
    ))?;
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(CredentialError::BadEnvironment(
            "YULIUS_TASK_TOKEN must be printable ASCII",
        ));
    }
    let endpoint = LoopbackUrl::parse(&base)?;
    let mut body = serde_json::Map::new();
    for (key, value) in [
        ("protocol", &request.protocol),
        ("host", &request.host),
        ("path", &request.path),
    ] {
        if let Some(value) = value {
            body.insert(key.to_owned(), serde_json::Value::String(value.clone()));
        }
    }
    let body = serde_json::Value::Object(body).to_string();
    let response = http_post(&endpoint, &token, &body, timeout)?;
    match response.status {
        200 => parse_credential(&response.body),
        401 | 403 | 404 => Err(CredentialError::Refused(response.status)),
        other => Err(CredentialError::Upstream(other)),
    }
}

fn write_credential(output: &mut dyn Write, credential: &Credential) -> std::io::Result<()> {
    let mut text = format!("username={TOKEN_USERNAME}\npassword={}\n", credential.token);
    if let Some(expiry) = credential.expires_at {
        text.push_str(&format!("password_expiry_utc={expiry}\n"));
    }
    output.write_all(text.as_bytes())?;
    output.flush()
}

fn parse_credential(body: &[u8]) -> Result<Credential, CredentialError> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| CredentialError::BadResponse("not JSON"))?;
    let token = value
        .get("token")
        .and_then(serde_json::Value::as_str)
        .ok_or(CredentialError::BadResponse("missing token"))?;
    if token.is_empty() || token.contains(['\n', '\r', '\0']) {
        return Err(CredentialError::BadResponse("token is empty or multi-line"));
    }
    let expires_at = value.get("expires_at").and_then(serde_json::Value::as_i64);
    Ok(Credential {
        token: token.to_owned(),
        expires_at,
    })
}

/// A parsed `http://<loopback>[:port][/base]` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LoopbackUrl {
    address: SocketAddr,
    host_header: String,
    base_path: String,
}

impl LoopbackUrl {
    fn parse(url: &str) -> Result<Self, CredentialError> {
        let invalid =
            || CredentialError::BadEnvironment("YULIUS_URL must be http:// on a loopback address");
        let rest = url.strip_prefix("http://").ok_or_else(invalid)?;
        if rest.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(invalid());
        }
        let (authority, path) = match rest.find('/') {
            Some(slash) => (&rest[..slash], &rest[slash..]),
            None => (rest, ""),
        };
        if authority.contains('@') || path.contains(['?', '#']) {
            return Err(invalid());
        }
        let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
            let (host, after) = bracketed.split_once(']').ok_or_else(invalid)?;
            let port = match after.strip_prefix(':') {
                Some(port) => port,
                None if after.is_empty() => "80",
                None => return Err(invalid()),
            };
            (host, port)
        } else {
            match authority.rsplit_once(':') {
                Some((host, port)) => (host, port),
                None => (authority, "80"),
            }
        };
        let port: u16 = port.parse().map_err(|_| invalid())?;
        let ip = if host.eq_ignore_ascii_case("localhost") {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        } else {
            host.parse::<IpAddr>().map_err(|_| invalid())?
        };
        let loopback = match ip {
            IpAddr::V4(v4) => v4.is_loopback(),
            IpAddr::V6(v6) => v6 == Ipv6Addr::LOCALHOST,
        };
        if !loopback {
            return Err(invalid());
        }
        Ok(Self {
            address: SocketAddr::new(ip, port),
            host_header: authority.to_owned(),
            base_path: path.trim_end_matches('/').to_owned(),
        })
    }
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

fn transport(context: &str, error: &std::io::Error) -> CredentialError {
    CredentialError::Transport(format!("{context}: {}", error.kind()))
}

fn remaining(deadline: Instant) -> Result<Duration, CredentialError> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(CredentialError::Transport("timed out".to_owned()))
    } else {
        Ok(left)
    }
}

fn http_post(
    endpoint: &LoopbackUrl,
    token: &str,
    body: &str,
    timeout: Duration,
) -> Result<HttpResponse, CredentialError> {
    let deadline = Instant::now() + timeout;
    let mut stream = TcpStream::connect_timeout(&endpoint.address, timeout)
        .map_err(|e| transport("connect", &e))?;
    let request = format!(
        "POST {}{CREDENTIAL_PATH} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\nUser-Agent: yulius-git-credential\r\n\r\n{body}",
        endpoint.base_path,
        endpoint.host_header,
        body.len()
    );
    stream
        .set_write_timeout(Some(remaining(deadline)?))
        .map_err(|e| transport("configure", &e))?;
    stream
        .write_all(request.as_bytes())
        .map_err(|e| transport("write", &e))?;
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(|e| transport("configure", &e))?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                raw.extend_from_slice(&chunk[..read]);
                if raw.len() > MAX_RESPONSE_BYTES {
                    return Err(CredentialError::BadResponse("response too large"));
                }
                if let Some(response) = try_parse_response(&raw)? {
                    return Ok(response);
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(transport("read", &error)),
        }
    }
    try_parse_response(&raw)?
        .or_else(|| parse_until_eof(&raw))
        .ok_or(CredentialError::BadResponse("truncated response"))
}

fn header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

fn parse_head(head: &[u8]) -> Result<(u16, Option<usize>, bool), CredentialError> {
    let text =
        std::str::from_utf8(head).map_err(|_| CredentialError::BadResponse("non-UTF-8 headers"))?;
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.split(' ');
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/1.") {
        return Err(CredentialError::BadResponse("not an HTTP/1.x response"));
    }
    let status = parts
        .next()
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or(CredentialError::BadResponse("bad status line"))?;
    let mut length = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| CredentialError::BadResponse("bad content-length"))?,
            );
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            && value.to_ascii_lowercase().contains("chunked")
        {
            chunked = true;
        }
    }
    Ok((status, length, chunked))
}

/// Returns a complete response once enough bytes have arrived, `None` if more are needed.
fn try_parse_response(raw: &[u8]) -> Result<Option<HttpResponse>, CredentialError> {
    let Some(end) = header_end(raw) else {
        return Ok(None);
    };
    let (status, length, chunked) = parse_head(&raw[..end - 4])?;
    let body = &raw[end..];
    if chunked {
        return Ok(decode_chunked(body)?.map(|body| HttpResponse { status, body }));
    }
    match length {
        Some(length) if body.len() >= length => Ok(Some(HttpResponse {
            status,
            body: body[..length].to_vec(),
        })),
        _ => Ok(None),
    }
}

/// Connection closed without a length: the body is everything after the headers.
fn parse_until_eof(raw: &[u8]) -> Option<HttpResponse> {
    let end = header_end(raw)?;
    let (status, length, chunked) = parse_head(&raw[..end - 4]).ok()?;
    if length.is_some() || chunked {
        return None;
    }
    Some(HttpResponse {
        status,
        body: raw[end..].to_vec(),
    })
}

/// Decodes a chunked body; `None` while incomplete.
fn decode_chunked(mut body: &[u8]) -> Result<Option<Vec<u8>>, CredentialError> {
    let mut decoded = Vec::new();
    loop {
        let Some(line_end) = body.windows(2).position(|w| w == b"\r\n") else {
            return Ok(None);
        };
        let size_text = std::str::from_utf8(&body[..line_end])
            .map_err(|_| CredentialError::BadResponse("bad chunk size"))?;
        let size_text = size_text.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| CredentialError::BadResponse("bad chunk size"))?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Ok(Some(decoded));
        }
        if body.len() < size + 2 {
            return Ok(None);
        }
        decoded.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn parses_loopback_urls_only() -> Result<(), CredentialError> {
        let url = LoopbackUrl::parse("http://127.0.0.1:7171/base/")?;
        assert_eq!(url.address.port(), 7171);
        assert_eq!(url.base_path, "/base");
        assert!(LoopbackUrl::parse("http://[::1]:80").is_ok());
        assert!(LoopbackUrl::parse("http://localhost:9").is_ok());
        for bad in [
            "https://127.0.0.1:1",
            "http://10.0.0.1:80",
            "http://example.com:80",
            "http://user@127.0.0.1:80",
            "http://127.0.0.1:99999",
            "http://0.0.0.0:80",
        ] {
            assert!(LoopbackUrl::parse(bad).is_err(), "{bad} accepted");
        }
        Ok(())
    }

    #[test]
    fn decodes_chunked_bodies() -> Result<(), CredentialError> {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"to\r\n9\r\nken\":\"t\"}\r\n0\r\n\r\n";
        let response =
            try_parse_response(raw)?.ok_or(CredentialError::BadResponse("incomplete"))?;
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"{\"token\":\"t\"}");
        assert!(try_parse_response(&raw[..raw.len() - 5])?.is_none());
        Ok(())
    }

    #[test]
    fn times_out_against_a_silent_server() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let server = thread::spawn(move || {
            // Accept and hold the connection open without answering until released.
            let held = listener.accept();
            let _ = released.recv();
            drop(held);
        });
        let env = move |key: &str| match key {
            "YULIUS_URL" => Some(format!("http://127.0.0.1:{port}")),
            "YULIUS_TASK_TOKEN" => Some("task-token".to_owned()),
            _ => None,
        };
        let request = CredentialRequest {
            protocol: Some("https".to_owned()),
            host: Some("github.com".to_owned()),
            path: None,
        };
        let started = Instant::now();
        let result = credential_get(&request, &env, Duration::from_millis(200));
        assert!(
            matches!(result, Err(CredentialError::Transport(_))),
            "{result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        release.send(())?;
        server.join().map_err(|_| "server thread panicked")?;
        Ok(())
    }
}
