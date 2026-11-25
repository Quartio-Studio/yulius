//! `yulius hook <harness> <event>`: forwards a CLI hook payload to the Yulius internal listener
//! (SPEC-002 §7.5, ADR-07).
//!
//! The client reads at most 1 MiB from stdin, POSTs it to
//! `${YULIUS_URL}/hooks/<harness>/<event>` with `Authorization: Bearer ${YULIUS_TASK_TOKEN}`
//! over a minimal HTTP/1.1 connection to a loopback address, and prints the body of a `200`
//! response. Every failure prints nothing and still exits 0 (fail-open), so a Yulius outage
//! never blocks the agent.
//!
//! **Hold** (subscription pool pause, SPEC-002 ADR-17): a `202 Accepted` answer with
//! `Retry-After: <seconds>` means "keep the agent at this boundary". The client sleeps that long
//! (1–60 s) and sends the same request again, until a `200` answers, the run is refused
//! (4xx), or [`HoldPolicy::max`] passes. Once held, an unreachable or failing server (a Yulius
//! restart, a 5xx) is retried every [`HoldPolicy::reconnect`] instead of failing open, so the
//! pause survives the restart. Each request keeps its own 5 s deadline.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use yulius_contracts::hooks::{INVOCATION_HEADER, SENT_AT_HEADER, events};
use yulius_domain::agent_endpoint::{AgentEndpoint, EndpointError};

/// Maximum hook payload read from stdin.
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;
/// Maximum `PreToolUse` input read: a `Write` carries the whole file, of which only the path is
/// sent on (SPEC-426 EDG-001).
pub const MAX_TOOL_INPUT_BYTES: usize = 16 * 1024 * 1024;
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
    /// No hold: a `202` fails open at once (a hook Yulius never holds, the edit gate).
    pub const NEVER: Self = Self {
        max: Duration::ZERO,
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
    // Why a write of harness memory fails, said in the sandbox (SPEC-652 MEM-006).
    if (harness == "claude" || harness == "codex")
        && event == yulius_contracts::hooks::MEMORY_GUARD_EVENT
    {
        return crate::memory_hook::run(input, output, env);
    }
    // Antigravity's hooks are translated to Claude Code's and back (SPEC-505 AGY-006).
    if harness == "antigravity" {
        return crate::antigravity_hooks::run(event, input, output, env, hold);
    }
    if harness == "claude" && event == "PermissionRequest" {
        return crate::cleanup_permission::answer(input, output);
    }
    // Tool results are redacted here, inside the sandbox, from fingerprints: the result itself
    // never goes to the server (SPEC-002 ADR-20).
    if harness == "claude" && event == "PostToolUse" {
        return crate::scrub_client::post_tool_use(input, output, env);
    }
    // Commands that can never work as meant are refused here, before they run; an edit of a
    // file goes to Yulius's edit gate (SPEC-426 EDG-001).
    // Codex's shell tool has Claude Code's shape (`Bash`, `tool_input.command`; SPEC-503).
    if (harness == "claude" || harness == "codex") && event == "PreToolUse" {
        let mut body = Vec::new();
        if input
            .take(MAX_TOOL_INPUT_BYTES as u64 + 1)
            .read_to_end(&mut body)
            .is_err()
        {
            return 0;
        }
        let Some(request) = crate::guard_client::edit_request(&body)
            .or_else(|| crate::guard_client::agent_request(&body))
        else {
            return crate::guard_client::pre_tool_use(&mut body.as_slice(), output);
        };
        if let Ok(answer) = forward(
            harness,
            event,
            &mut request.as_slice(),
            env,
            HOOK_TIMEOUT,
            HoldPolicy::NEVER,
        ) {
            let _ignored = output.write_all(&answer).and_then(|()| output.flush());
        }
        return 0;
    }
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
    let target = AgentEndpoint::from_env(env).map_err(HookError::from)?;
    let path = format!("/hooks/{harness}/{event}");
    let request = Request {
        target: &target,
        path: &path,
        body: &body,
        invocation: (event == events::USER_PROMPT_SUBMIT).then(uuid::Uuid::new_v4),
        timeout,
    };
    let started = Instant::now();
    let mut held = false;
    loop {
        let wait = match ask(&request) {
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

/// POSTs `body` to `${YULIUS_URL}<path>` with the task token (no hold): the internal
/// endpoints the in-sandbox redaction uses.
pub(crate) fn post_internal(
    path: &str,
    body: &[u8],
    env: &dyn Fn(&str) -> Option<String>,
    timeout: Duration,
) -> Result<Vec<u8>, HookError> {
    let target = AgentEndpoint::from_env(env).map_err(HookError::from)?;
    let request = Request {
        target: &target,
        path,
        body,
        invocation: None,
        timeout,
    };
    match ask(&request)? {
        Answer::Output(response) => Ok(response),
        Answer::Hold(_) => Err(HookError::Protocol),
    }
}

/// One hook request's fixed parts.
struct Request<'a> {
    target: &'a AgentEndpoint,
    path: &'a str,
    body: &'a [u8],
    invocation: Option<uuid::Uuid>,
    timeout: Duration,
}

/// Sends the request to the first address that answers.
fn ask(request: &Request<'_>) -> Result<Answer, HookError> {
    let mut last_error = HookError::Io;
    for addr in &request.target.addresses {
        match exchange(addr, request) {
            Ok(answer) => return Ok(answer),
            Err(error @ (HookError::Status(_) | HookError::Protocol)) => return Err(error),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

impl From<EndpointError> for HookError {
    fn from(error: EndpointError) -> Self {
        match error {
            EndpointError::UrlMissing
            | EndpointError::TokenMissing
            | EndpointError::TokenNotPrintable => Self::MissingEnv,
            EndpointError::Url => Self::Url,
            EndpointError::NotLoopback => Self::NotLoopback,
        }
    }
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
        path,
        body,
        invocation,
        timeout,
    } = *request;
    let token = &target.token;
    let stream = TcpStream::connect_timeout(addr, timeout)?;
    let deadline = Instant::now() + timeout;
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    // When the agent sent the hook, so Yulius orders a hook it handles late (SPEC-346 LPH-001).
    let sent_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let invocation_header = invocation.map_or_else(String::new, |id| {
        format!("{INVOCATION_HEADER}: {}\r\n", id.simple())
    });
    let head = format!(
        "POST {}{path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n{SENT_AT_HEADER}: {sent_ms}\r\n\
         {invocation_header}Connection: close\r\n\r\n",
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
    if status == 204 {
        return Ok(Answer::Output(Vec::new()));
    }
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
    // Closed before any answer (a relay at its limit, a server going down): unreachable, not an
    // answer off the protocol (SPEC-353 LAP-001).
    if *head_bytes == 0 {
        return Err(HookError::Io);
    }
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

    /// A listener on ::1 whose port nothing holds on 127.0.0.1 (another test's listener there
    /// would take the first connection): tried a few times, as ports are given at random.
    fn ipv6_only_listener() -> std::io::Result<TcpListener> {
        for _ in 0..20 {
            let listener = TcpListener::bind("[::1]:0")?;
            let port = listener.local_addr()?.port();
            if TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return Ok(listener);
            }
        }
        Err(std::io::Error::other(
            "no port free on both loopback addresses",
        ))
    }

    /// SPEC-483 slice 34 (review): `localhost` is 127.0.0.1 then ::1; the hook client reaches a
    /// server on ::1 only through the second address.
    #[test]
    fn localhost_reaches_a_server_on_the_second_address() -> io::Result<()> {
        let listener = ipv6_only_listener()?;
        let port = listener.local_addr()?.port();
        let server = thread::spawn(move || -> io::Result<()> {
            let (mut stream, _) = listener.accept()?;
            let mut buf = [0u8; 4096];
            let _read = stream.read(&mut buf)?;
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")?;
            Ok(())
        });
        let url = format!("http://localhost:{port}");
        let env = move |key: &str| match key {
            "YULIUS_URL" => Some(url.clone()),
            "YULIUS_TASK_TOKEN" => Some("t".into()),
            _ => None,
        };
        let answer = post_internal("/internal/x", b"{}", &env, Duration::from_secs(5));
        assert_eq!(answer.ok().as_deref(), Some(&b"ok"[..]));
        server
            .join()
            .map_err(|_| io::Error::other("server panicked"))?
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
