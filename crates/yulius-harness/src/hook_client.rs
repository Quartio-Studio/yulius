//! `yulius hook <harness> <event>`: forwards a CLI hook payload to the Yulius internal listener
//! (SPEC-002 §7.5, ADR-07).
//!
//! The client reads at most 1 MiB from stdin, POSTs it to
//! `${YULIUS_URL}/hooks/<harness>/<event>` with `Authorization: Bearer ${YULIUS_TASK_TOKEN}`
//! over a minimal HTTP/1.1 connection to a loopback address, and prints the body of a `200`
//! response. Every failure prints nothing and still exits 0 (fail-open), so a Yulius outage
//! never blocks the agent.
//!
//! **Hold** (subscription pool pause, SPEC-002 ADR-16): a `202 Accepted` answer with
//! `Retry-After: <seconds>` means "keep the agent at this boundary". The client sleeps that long
//! (1–60 s) and sends the same request again, until a `200` answers, the run is refused
//! (4xx), or [`HoldPolicy::max`] passes. Once held, an unreachable or failing server (a Yulius
//! restart, a 5xx) is retried every [`HoldPolicy::reconnect`] instead of failing open, so the
//! pause survives the restart. Each request keeps its own 5 s deadline.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::launch::{TASK_TOKEN_ENV, YULIUS_URL_ENV};

/// Maximum hook payload read from stdin.
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;
/// Maximum response body accepted.
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// Maximum response head (status line and headers).
const MAX_HEAD_BYTES: usize = 64 * 1024;
/// Connect timeout and, separately, the deadline for sending the request and reading the
/// response (SPEC-002 §9: hook call 5 s).
pub const HOOK_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest a held hook keeps the agent waiting before it fails open: longer than a weekly
/// window, shorter than the CLI's own timeout for held events
/// ([`crate::launch::CLAUDE_HELD_HOOK_TIMEOUT_SECONDS`]).
pub const HOLD_MAX: Duration = Duration::from_secs(8 * 24 * 3600);
/// Upper bound of the server's `Retry-After` between two requests of a held hook.
const HOLD_RETRY_MAX: Duration = Duration::from_secs(60);

/// How a held hook waits (production: [`HoldPolicy::DEFAULT`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoldPolicy {
    /// Total time a hook may stay held before it fails open.
    pub max: Duration,
    /// Shortest wait between two requests, whatever `Retry-After` says.
    pub min_wait: Duration,
    /// Wait before asking again when Yulius cannot be reached or fails during a hold.
    pub reconnect: Duration,
}

impl HoldPolicy {
    /// Production bounds: [`HOLD_MAX`], at least 1 s between requests, 2 s after a failure.
    pub const DEFAULT: Self = Self {
        max: HOLD_MAX,
        min_wait: Duration::from_secs(1),
        reconnect: Duration::from_secs(2),
    };
}

/// Runs one hook invocation. Always returns exit code 0.
///
/// `env` looks up `YULIUS_URL` and `YULIUS_TASK_TOKEN`. Output is written only for a `200`
/// response.
pub fn run_hook(
    harness: &str,
    event: &str,
    input: &mut dyn Read,
    output: &mut dyn Write,
    env: &dyn Fn(&str) -> Option<String>,
) -> i32 {
    run_hook_with(harness, event, input, output, env, HoldPolicy::DEFAULT)
}

/// [`run_hook`] with explicit hold bounds (tests).
pub fn run_hook_with(
    harness: &str,
    event: &str,
    input: &mut dyn Read,
    output: &mut dyn Write,
    env: &dyn Fn(&str) -> Option<String>,
    hold: HoldPolicy,
) -> i32 {
    if let Ok(body) = forward(harness, event, input, env, HOOK_TIMEOUT, hold) {
        let _ignored = output.write_all(&body).and_then(|()| output.flush());
    }
    0
}

/// What one request was answered with.
enum Answer {
    /// `200`: print this body.
    Output(Vec<u8>),
    /// `202`: ask again after this long.
    Hold(Duration),
}

#[derive(Debug)]
pub(crate) enum HookError {
    InvalidName,
    InputTooLarge,
    MissingEnv,
    Url,
    NotLoopback,
    Io,
    Status(u16),
    Protocol,
    HoldExpired,
}

impl From<io::Error> for HookError {
    fn from(_: io::Error) -> Self {
        Self::Io
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric())
}

pub(crate) fn forward(
    harness: &str,
    event: &str,
    input: &mut dyn Read,
    env: &dyn Fn(&str) -> Option<String>,
    timeout: Duration,
    hold: HoldPolicy,
) -> Result<Vec<u8>, HookError> {
    if !valid_name(harness) || !valid_name(event) {
        return Err(HookError::InvalidName);
    }
    let mut body = Vec::new();
    input
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut body)?;
    if body.len() > MAX_INPUT_BYTES {
        return Err(HookError::InputTooLarge);
    }
    let url = env(YULIUS_URL_ENV).ok_or(HookError::MissingEnv)?;
    let token = env(TASK_TOKEN_ENV).ok_or(HookError::MissingEnv)?;
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(HookError::MissingEnv);
    }
    let target = parse_url(&url)?;
    let addrs: Vec<SocketAddr> = (target.host.as_str(), target.port)
        .to_socket_addrs()
        .map_err(|_| HookError::Url)?
        .collect();
    if addrs.is_empty() || !addrs.iter().all(|a| a.ip().is_loopback()) {
        return Err(HookError::NotLoopback);
    }
    let request = Request {
        target: &target,
        harness,
        event,
        token: &token,
        body: &body,
        timeout,
    };
    let started = Instant::now();
    let mut held = false;
    loop {
        let wait = match ask(&addrs, &request) {
            Ok(Answer::Output(response)) => return Ok(response),
            Ok(Answer::Hold(retry_after)) => {
                held = true;
                retry_after.clamp(hold.min_wait, HOLD_RETRY_MAX.max(hold.min_wait))
            }
            // Held: Yulius restarting (unreachable) or failing for a moment keeps the hold.
            Err(HookError::Io) if held => hold.reconnect,
            Err(HookError::Status(code)) if held && (500..600).contains(&code) => hold.reconnect,
            Err(error) => return Err(error),
        };
        if started.elapsed().saturating_add(wait) > hold.max {
            return Err(HookError::HoldExpired);
        }
        std::thread::sleep(wait);
    }
}

/// One hook request's fixed parts.
struct Request<'a> {
    target: &'a Target,
    harness: &'a str,
    event: &'a str,
    token: &'a str,
    body: &'a [u8],
    timeout: Duration,
}

/// Sends the request to the first address that answers.
fn ask(addrs: &[SocketAddr], request: &Request<'_>) -> Result<Answer, HookError> {
    let mut last_error = HookError::Io;
    for addr in addrs {
        match exchange(addr, request) {
            Ok(answer) => return Ok(answer),
            Err(error @ (HookError::Status(_) | HookError::Protocol)) => return Err(error),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Target {
    host: String,
    port: u16,
    base_path: String,
    host_header: String,
}

/// Parses `http://host[:port][/path]`; only plain HTTP to a literal loopback address or
/// `localhost` is accepted.
pub(crate) fn parse_url(url: &str) -> Result<Target, HookError> {
    let rest = url.strip_prefix("http://").ok_or(HookError::Url)?;
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    if authority.is_empty() || authority.contains('@') || path.contains(['?', '#']) {
        return Err(HookError::Url);
    }
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, after) = bracketed.split_once(']').ok_or(HookError::Url)?;
        let port = match after {
            "" => 80,
            p => p
                .strip_prefix(':')
                .and_then(|p| p.parse().ok())
                .ok_or(HookError::Url)?,
        };
        (host, port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().map_err(|_| HookError::Url)?),
            None => (authority, 80),
        }
    };
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    if !loopback {
        return Err(HookError::NotLoopback);
    }
    if !path.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(HookError::Url);
    }
    Ok(Target {
        host: host.to_owned(),
        port,
        base_path: path.trim_end_matches('/').to_owned(),
        host_header: authority.to_owned(),
    })
}

fn remaining(deadline: Instant) -> Result<Duration, HookError> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(HookError::Io)
    } else {
        Ok(left)
    }
}

fn exchange(addr: &SocketAddr, request: &Request<'_>) -> Result<Answer, HookError> {
    let Request {
        target,
        harness,
        event,
        token,
        body,
        timeout,
    } = *request;
    let stream = TcpStream::connect_timeout(addr, timeout)?;
    let deadline = Instant::now() + timeout;
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    let head = format!(
        "POST {}/hooks/{harness}/{event} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        target.base_path,
        target.host_header,
        body.len()
    );
    let mut writer = &stream;
    writer.write_all(head.as_bytes())?;
    writer.write_all(body)?;
    writer.flush()?;

    let mut reader = BufReader::new(Deadline {
        stream: &stream,
        deadline,
    });
    let mut head_bytes = 0usize;
    let mut status_line = String::new();
    read_head_line(&mut reader, &mut status_line, &mut head_bytes)?;
    let mut parts = status_line.split_whitespace();
    let version = parts.next().unwrap_or_default();
    let code = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/1.") {
        return Err(HookError::Protocol);
    }
    let status: u16 = code.parse().map_err(|_| HookError::Protocol)?;
    if status != 200 && status != 202 {
        return Err(HookError::Status(status));
    }
    let mut content_length: Option<usize> = None;
    let mut retry_after: Option<Duration> = None;
    loop {
        let mut line = String::new();
        read_head_line(&mut reader, &mut line, &mut head_bytes)?;
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(HookError::Protocol);
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            let length: usize = value.parse().map_err(|_| HookError::Protocol)?;
            if length > MAX_RESPONSE_BYTES {
                return Err(HookError::Protocol);
            }
            content_length = Some(length);
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            && !value.eq_ignore_ascii_case("identity")
        {
            return Err(HookError::Protocol);
        } else if name.eq_ignore_ascii_case("retry-after") {
            retry_after = value.parse::<u64>().ok().map(Duration::from_secs);
        }
    }
    if status == 202 {
        // A hold: the body (if any) is not hook output.
        return Ok(Answer::Hold(retry_after.unwrap_or(HOLD_RETRY_MAX)));
    }
    let mut response = Vec::new();
    match content_length {
        Some(length) => {
            response.resize(length, 0);
            reader.read_exact(&mut response)?;
        }
        None => {
            reader
                .take(MAX_RESPONSE_BYTES as u64 + 1)
                .read_to_end(&mut response)?;
            if response.len() > MAX_RESPONSE_BYTES {
                return Err(HookError::Protocol);
            }
        }
    }
    Ok(Answer::Output(response))
}

fn read_head_line(
    reader: &mut impl BufRead,
    line: &mut String,
    head_bytes: &mut usize,
) -> Result<(), HookError> {
    let budget = MAX_HEAD_BYTES.saturating_sub(*head_bytes);
    let read = reader.take(budget as u64).read_line(line)?;
    *head_bytes += read;
    if read == 0 || !line.ends_with('\n') {
        return Err(HookError::Protocol);
    }
    Ok(())
}

/// A reader that applies the remaining time to the deadline before every read.
struct Deadline<'a> {
    stream: &'a TcpStream,
    deadline: Instant,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "hook deadline"));
        }
        self.stream.set_read_timeout(Some(left))?;
        let mut stream = self.stream;
        stream.read(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn url_parsing_accepts_only_loopback_http() -> Result<(), String> {
        let ok = parse_url("http://127.0.0.1:8123").map_err(|e| format!("{e:?}"))?;
        assert_eq!(ok.port, 8123);
        assert_eq!(ok.base_path, "");
        let prefixed = parse_url("http://localhost:9/internal/").map_err(|e| format!("{e:?}"))?;
        assert_eq!(prefixed.base_path, "/internal");
        let v6 = parse_url("http://[::1]:7").map_err(|e| format!("{e:?}"))?;
        assert_eq!((v6.host.as_str(), v6.port), ("::1", 7));
        assert!(matches!(
            parse_url("http://10.0.0.1:80"),
            Err(HookError::NotLoopback)
        ));
        assert!(matches!(
            parse_url("http://example.com"),
            Err(HookError::NotLoopback)
        ));
        assert!(matches!(
            parse_url("https://127.0.0.1"),
            Err(HookError::Url)
        ));
        assert!(matches!(
            parse_url("http://u@127.0.0.1"),
            Err(HookError::Url)
        ));
        assert!(matches!(
            parse_url("http://127.0.0.1:x"),
            Err(HookError::Url)
        ));
        Ok(())
    }

    #[test]
    fn slow_server_hits_the_deadline() -> io::Result<()> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let server = thread::spawn(move || -> io::Result<()> {
            let (mut stream, _) = listener.accept()?;
            let mut buf = [0u8; 4096];
            let _read = stream.read(&mut buf)?;
            // Send a partial head and then stall until the client gives up.
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n")?;
            let _rest = stream.read(&mut buf);
            Ok(())
        });
        let url = format!("http://127.0.0.1:{port}");
        let env = move |key: &str| match key {
            "YULIUS_URL" => Some(url.clone()),
            "YULIUS_TASK_TOKEN" => Some("t".into()),
            _ => None,
        };
        let started = Instant::now();
        let result = forward(
            "claude",
            "Stop",
            &mut &b"{}"[..],
            &env,
            Duration::from_millis(200),
            HoldPolicy::DEFAULT,
        );
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
        server
            .join()
            .map_err(|_| io::Error::other("server panicked"))??;
        Ok(())
    }
}
