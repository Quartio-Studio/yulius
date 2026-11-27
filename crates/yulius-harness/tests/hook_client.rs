//! Hook client contracts against an in-test loopback HTTP server: forwarding, fail-open
//! behavior and bounds (SPEC-002 §7.5, §9 hook call 5 s).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};

use std::time::{Duration, Instant};

use yulius_harness::hook_client::{HoldPolicy, MAX_INPUT_BYTES, run_hook, run_hook_with};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Debug, Default)]
struct Request {
    line: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> io::Result<Request> {
    let mut reader = BufReader::new(stream);
    let mut request = Request::default();
    reader.read_line(&mut request.line)?;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            request
                .headers
                .insert(name.to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let length: usize = request
        .headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    request.body.resize(length, 0);
    reader.read_exact(&mut request.body)?;
    Ok(request)
}

/// Serves exactly one connection with a canned raw response and returns what it received.
fn serve_once(response: Vec<u8>) -> io::Result<(String, JoinHandle<io::Result<Request>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept()?;
        let request = read_request(&mut stream)?;
        stream.write_all(&response)?;
        Ok(request)
    });
    Ok((url, handle))
}

/// [`serve_once`] that stops waiting for the connection after `within` (an error, not a hang).
fn serve_once_within(
    response: Vec<u8>,
    within: Duration,
) -> io::Result<(String, JoinHandle<io::Result<Request>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + within;
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => return Err(e),
            }
        };
        stream.set_nonblocking(false)?;
        let request = read_request(&mut stream)?;
        stream.write_all(&response)?;
        Ok(request)
    });
    Ok((url, handle))
}

fn env_with(url: String, token: Option<&'static str>) -> impl Fn(&str) -> Option<String> {
    move |key| match key {
        "YULIUS_URL" => Some(url.clone()),
        "YULIUS_TASK_TOKEN" => token.map(str::to_owned),
        _ => None,
    }
}

fn join(handle: JoinHandle<io::Result<Request>>) -> io::Result<Request> {
    handle
        .join()
        .map_err(|_| io::Error::other("server thread panicked"))?
}

/// A listener that must never receive a connection.
/// A URL nothing answers on, whose port stays taken until the guard drops: a socket bound to it
/// that never listens refuses every connection. A port a test only frees could be bound by a
/// test running in parallel meanwhile, whose server would then take this test's request (#1693).
fn refused_url() -> io::Result<(String, std::os::fd::OwnedFd)> {
    use nix::sys::socket::{
        AddressFamily, SockFlag, SockType, SockaddrIn, bind, getsockname, socket,
    };
    let socket = socket(
        AddressFamily::Inet,
        SockType::Stream,
        SockFlag::SOCK_CLOEXEC,
        None,
    )?;
    bind(
        std::os::fd::AsRawFd::as_raw_fd(&socket),
        &SockaddrIn::new(127, 0, 0, 1, 0),
    )?;
    let bound: SockaddrIn = getsockname(std::os::fd::AsRawFd::as_raw_fd(&socket))?;
    Ok((format!("http://127.0.0.1:{}", bound.port()), socket))
}

/// The port of a [`refused_url`] stays taken while its guard lives, and refuses connections.
#[test]
fn a_refused_url_keeps_its_port_and_refuses() -> TestResult {
    let (url, guard) = refused_url()?;
    let port = url.rsplit(':').next().ok_or("port")?.parse::<u16>()?;
    assert!(
        TcpListener::bind(("127.0.0.1", port)).is_err(),
        "no other socket gets the port"
    );
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
    drop(guard);
    Ok(())
}

fn untouched_listener() -> io::Result<(String, TcpListener)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    Ok((url, listener))
}

fn assert_no_connection(listener: &TcpListener) {
    assert!(
        matches!(listener.accept(), Err(e) if e.kind() == io::ErrorKind::WouldBlock),
        "the hook client must not have connected"
    );
}

#[test]
fn forwards_payload_and_prints_200_body() -> TestResult {
    let body = br#"{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"briefing"}}"#;
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    let (url, server) = serve_once(response)?;
    let payload = br#"{"session_id":"s","hook_event_name":"SessionStart"}"#;
    let mut output = Vec::new();
    let before = unix_ms();
    let code = run_hook(
        "claude",
        "SessionStart",
        &mut &payload[..],
        &mut output,
        &env_with(url, Some("task-token-SYNTHETIC")),
    );
    assert_eq!(code, 0);
    assert_eq!(output, body);
    let request = join(server)?;
    assert_eq!(
        request.line.trim_end(),
        "POST /hooks/claude/SessionStart HTTP/1.1"
    );
    assert_eq!(
        request.headers.get("authorization").map(String::as_str),
        Some("Bearer task-token-SYNTHETIC")
    );
    assert_eq!(
        request.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    assert_eq!(request.body, payload);
    // When it was sent, so Yulius orders a hook it handles late (SPEC-346 LPH-001).
    let sent: u128 = request
        .headers
        .get("x-yulius-hook-sent-ms")
        .ok_or("no sent time")?
        .parse()?;
    assert!((before..=unix_ms()).contains(&sent), "{sent}");
    Ok(())
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

#[test]
fn base_path_and_body_without_content_length() -> TestResult {
    let (url, server) = serve_once(
        b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{\"decision\":\"block\"}".to_vec(),
    )?;
    let mut output = Vec::new();
    let code = run_hook(
        "claude",
        "Stop",
        &mut &b"{}"[..],
        &mut output,
        &env_with(format!("{url}/internal/"), Some("t")),
    );
    assert_eq!(code, 0);
    assert_eq!(output, b"{\"decision\":\"block\"}");
    assert_eq!(
        join(server)?.line.trim_end(),
        "POST /internal/hooks/claude/Stop HTTP/1.1"
    );
    Ok(())
}

#[test]
fn non_200_and_unsupported_framing_print_nothing() -> TestResult {
    for response in [
        b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 5\r\n\r\nnope!".to_vec(),
        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n".to_vec(),
        b"HTTP/1.1 204 No Content\r\n\r\n".to_vec(),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 999999999\r\n\r\n".to_vec(),
        b"SSH-2.0-not-http\r\n\r\n".to_vec(),
    ] {
        let (url, server) = serve_once(response)?;
        let mut output = Vec::new();
        let code = run_hook(
            "claude",
            "Stop",
            &mut &b"{}"[..],
            &mut output,
            &env_with(url, Some("t")),
        );
        assert_eq!(code, 0);
        assert!(output.is_empty());
        join(server)?;
    }
    Ok(())
}

#[test]
fn unreachable_server_is_fail_open() -> TestResult {
    let (url, _refusing) = refused_url()?;
    let mut output = Vec::new();
    let code = run_hook(
        "claude",
        "Stop",
        &mut &b"{}"[..],
        &mut output,
        &env_with(url, Some("t")),
    );
    assert_eq!(code, 0);
    assert!(output.is_empty());
    Ok(())
}

#[test]
fn refuses_non_loopback_and_bad_configuration_without_connecting() -> TestResult {
    let mut output = Vec::new();
    for url in [
        "http://192.0.2.1:80",
        "http://example.com",
        "https://127.0.0.1:1",
    ] {
        let code = run_hook(
            "claude",
            "Stop",
            &mut &b"{}"[..],
            &mut output,
            &env_with(url.into(), Some("t")),
        );
        assert_eq!(code, 0);
    }
    assert!(output.is_empty());

    let (url, listener) = untouched_listener()?;
    // Missing token.
    run_hook(
        "claude",
        "Stop",
        &mut &b"{}"[..],
        &mut output,
        &env_with(url.clone(), None),
    );
    // Token that could inject headers.
    run_hook(
        "claude",
        "Stop",
        &mut &b"{}"[..],
        &mut output,
        &env_with(url.clone(), Some("a\r\nX-Evil: 1")),
    );
    // Path-injecting event name.
    run_hook(
        "claude",
        "../admin",
        &mut &b"{}"[..],
        &mut output,
        &env_with(url.clone(), Some("t")),
    );
    // Oversized payload is not forwarded truncated.
    let big = vec![b' '; MAX_INPUT_BYTES + 1];
    let code = run_hook(
        "claude",
        "Stop",
        &mut &big[..],
        &mut output,
        &env_with(url, Some("t")),
    );
    assert_eq!(code, 0);
    assert!(output.is_empty());
    assert_no_connection(&listener);
    // Missing URL.
    let no_url = |_: &str| None;
    assert_eq!(
        run_hook("claude", "Stop", &mut &b"{}"[..], &mut output, &no_url),
        0
    );
    Ok(())
}

#[test]
fn payload_at_the_bound_is_forwarded() -> TestResult {
    let (url, server) = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".to_vec())?;
    let payload = vec![b' '; MAX_INPUT_BYTES];
    let mut output = Vec::new();
    run_hook(
        "claude",
        "Stop",
        &mut &payload[..],
        &mut output,
        &env_with(url, Some("t")),
    );
    assert_eq!(output, b"{}");
    assert_eq!(join(server)?.body.len(), MAX_INPUT_BYTES);
    Ok(())
}

struct FailingWriter;

impl Write for FailingWriter {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("closed stdout"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("closed stdout"))
    }
}

#[test]
fn output_errors_still_exit_zero() -> TestResult {
    let (url, server) = serve_once(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}".to_vec())?;
    let code = run_hook(
        "claude",
        "Stop",
        &mut &b"{}"[..],
        &mut FailingWriter,
        &env_with(url, Some("t")),
    );
    assert_eq!(code, 0);
    join(server)?;
    Ok(())
}

/// Fast hold bounds for tests.
const FAST_HOLD: HoldPolicy = HoldPolicy {
    max: Duration::from_secs(20),
    min_wait: Duration::from_millis(10),
    reconnect: Duration::from_millis(50),
};

/// Serves `responses` in order on one listener, one connection each, and returns the requests.
fn serve_sequence(
    listener: TcpListener,
    responses: Vec<Vec<u8>>,
) -> JoinHandle<io::Result<Vec<Request>>> {
    thread::spawn(move || {
        let mut requests = Vec::new();
        for response in responses {
            let (mut stream, _) = listener.accept()?;
            requests.push(read_request(&mut stream)?);
            stream.write_all(&response)?;
        }
        Ok(requests)
    })
}

const HOLD: &[u8] = b"HTTP/1.1 202 Accepted\r\nRetry-After: 0\r\nContent-Length: 0\r\n\r\n";

/// ADR-17: `202` + `Retry-After` holds the agent at its boundary: the same request is sent again
/// until a `200` answers, and only that answer is printed.
#[test]
fn a_held_hook_asks_again_and_prints_only_the_final_answer() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let server = serve_sequence(
        listener,
        vec![
            HOLD.to_vec(),
            HOLD.to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 17\r\n\r\n{\"decision\":null}".to_vec(),
        ],
    );
    let mut output = Vec::new();
    let payload = br#"{"hook_event_name":"UserPromptSubmit"}"#;
    let code = run_hook_with(
        "claude",
        "UserPromptSubmit",
        &mut &payload[..],
        &mut output,
        &env_with(url, Some("t")),
        FAST_HOLD,
    );
    assert_eq!(code, 0);
    assert_eq!(output, br#"{"decision":null}"#);
    let requests = server
        .join()
        .map_err(|_| io::Error::other("server thread panicked"))??;
    assert_eq!(requests.len(), 3, "asked again after each hold");
    assert!(
        requests.iter().all(|r| r.body == payload),
        "the same payload"
    );
    let invocation = requests[0]
        .headers
        .get("x-yulius-hook-invocation")
        .ok_or("missing submit invocation")?;
    assert_eq!(invocation.len(), 32);
    assert!(
        requests
            .iter()
            .all(|r| r.headers.get("x-yulius-hook-invocation") == Some(invocation))
    );
    Ok(())
}

/// ADR-17: a hold survives a Yulius restart — while held, a refused connection and a 5xx are
/// retried instead of failing open, and the restarted server's answer is printed.
#[test]
fn a_held_hook_survives_a_server_restart() -> TestResult {
    let first = TcpListener::bind("127.0.0.1:0")?;
    let port = first.local_addr()?.port();
    let url = format!("http://127.0.0.1:{port}");
    let before = serve_sequence(first, vec![HOLD.to_vec()]);
    let client = thread::spawn(move || {
        let mut output = Vec::new();
        run_hook_with(
            "claude",
            "Stop",
            &mut &b"{}"[..],
            &mut output,
            &env_with(url, Some("t")),
            FAST_HOLD,
        );
        output
    });
    before
        .join()
        .map_err(|_| io::Error::other("server thread panicked"))??;
    // The server is down for a while (connections refused), then comes back on the same port,
    // first failing once, then answering.
    thread::sleep(Duration::from_millis(300));
    let again = TcpListener::bind(("127.0.0.1", port))?;
    let after = serve_sequence(
        again,
        vec![
            b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n".to_vec(),
            HOLD.to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec(),
        ],
    );
    let output = client
        .join()
        .map_err(|_| io::Error::other("client thread panicked"))?;
    assert_eq!(output, b"ok");
    after
        .join()
        .map_err(|_| io::Error::other("server thread panicked"))??;
    Ok(())
}

/// SPEC-353 (review round 2): in a sandbox Yulius is behind the relay, which accepts a connection
/// and closes it unanswered while Yulius restarts or the relay is at its limit. Held, that close is
/// retried as an unreachable server is, and the later answer is printed.
#[test]
fn a_held_hook_retries_a_connection_closed_unanswered() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let server = serve_sequence(
        listener,
        vec![
            HOLD.to_vec(),
            Vec::new(), // read, then closed with no answer
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec(),
        ],
    );
    let mut output = Vec::new();
    run_hook_with(
        "claude",
        "Stop",
        &mut &b"{}"[..],
        &mut output,
        &env_with(url, Some("t")),
        FAST_HOLD,
    );
    assert_eq!(output, b"ok");
    let requests = server
        .join()
        .map_err(|_| io::Error::other("server thread panicked"))??;
    assert_eq!(requests.len(), 3);
    Ok(())
}

/// A held hook fails open (prints nothing) when the run is refused or the hold bound passes.
#[test]
fn a_held_hook_fails_open_on_refusal_or_after_its_bound() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let server = serve_sequence(
        listener,
        vec![
            HOLD.to_vec(),
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_vec(),
        ],
    );
    let mut output = Vec::new();
    run_hook_with(
        "claude",
        "Stop",
        &mut &b"{}"[..],
        &mut output,
        &env_with(url, Some("t")),
        FAST_HOLD,
    );
    assert!(output.is_empty(), "a finished run fails open");
    server
        .join()
        .map_err(|_| io::Error::other("server thread panicked"))??;

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let server = serve_sequence(listener, vec![HOLD.to_vec(); 3]);
    let started = Instant::now();
    let mut output = Vec::new();
    run_hook_with(
        "claude",
        "Stop",
        &mut &b"{}"[..],
        &mut output,
        &env_with(url, Some("t")),
        HoldPolicy {
            max: Duration::from_millis(150),
            min_wait: Duration::from_millis(60),
            reconnect: Duration::from_millis(60),
        },
    );
    assert!(output.is_empty(), "the hold bound fails open");
    assert!(started.elapsed() < Duration::from_secs(5));
    // The client stopped asking before the third hold.
    drop(server);
    Ok(())
}

/// Without a prior hold, an unreachable server still fails open at once (no retry loop).
#[test]
fn an_unheld_hook_does_not_retry() -> TestResult {
    let (url, _refusing) = refused_url()?;
    let started = Instant::now();
    let mut output = Vec::new();
    run_hook_with(
        "claude",
        "Stop",
        &mut &b"{}"[..],
        &mut output,
        &env_with(url, Some("t")),
        FAST_HOLD,
    );
    assert!(output.is_empty());
    assert!(started.elapsed() < Duration::from_secs(3));
    Ok(())
}

/// SPEC-503 CDX-007: Codex's shell tool has Claude Code's shape (`Bash`, `tool_input.command`),
/// so a Codex run's `PreToolUse` gets the same local guard: a command that can never work as
/// meant is denied in the sandbox, without a connection.
#[test]
fn a_codex_bash_call_is_checked_locally_like_claude_codes() -> TestResult {
    let (url, listener) = untouched_listener()?;
    let env = env_with(url, Some("task-token-SYNTHETIC"));
    let mut output = Vec::new();
    let bash = br#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"pkill -f cargo-nextest"}}"#;
    run_hook("codex", "PreToolUse", &mut &bash[..], &mut output, &env);
    assert!(
        String::from_utf8_lossy(&output).contains("\"permissionDecision\":\"deny\""),
        "{}",
        String::from_utf8_lossy(&output)
    );
    assert_no_connection(&listener);
    Ok(())
}

/// SPEC-426 EDG-001: an edit tool's `PreToolUse` goes to Yulius (its edit gate) and its answer is
/// printed; a Bash command is still checked in the sandbox, without a connection.
#[test]
fn pre_tool_use_sends_edits_to_yulius_and_checks_bash_locally() -> TestResult {
    let body = br#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"write the Plan"}}"#;
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    let (url, server) = serve_once(response)?;
    let payload = br#"{"hook_event_name":"PreToolUse","tool_name":"Edit","cwd":"/w","session_id":"s","tool_input":{"file_path":"/w/api.rs","old_string":"secret text","new_string":"more"}}"#;
    let mut output = Vec::new();
    let code = run_hook(
        "claude",
        "PreToolUse",
        &mut &payload[..],
        &mut output,
        &env_with(url, Some("task-token-SYNTHETIC")),
    );
    assert_eq!(code, 0);
    assert_eq!(output, body);
    let request = join(server)?;
    assert_eq!(
        request.line.trim_end(),
        "POST /hooks/claude/PreToolUse HTTP/1.1"
    );
    // Only what the edit gate reads leaves the sandbox: never the file's content.
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&request.body)?,
        serde_json::json!({"hook_event_name": "PreToolUse", "tool_name": "Edit", "cwd": "/w",
                           "tool_input": {"file_path": "/w/api.rs"}})
    );

    let (url, listener) = untouched_listener()?;
    let env = env_with(url, Some("task-token-SYNTHETIC"));
    let mut output = Vec::new();
    let bash = br#"{"tool_name":"Bash","tool_input":{"command":"pkill -f cargo-nextest"}}"#;
    run_hook("claude", "PreToolUse", &mut &bash[..], &mut output, &env);
    assert!(
        String::from_utf8_lossy(&output).contains("\"permissionDecision\":\"deny\""),
        "a self-matching pkill is denied locally"
    );
    let mut output = Vec::new();
    let read = br#"{"tool_name":"Read","tool_input":{"file_path":"/w/api.rs"}}"#;
    run_hook("claude", "PreToolUse", &mut &read[..], &mut output, &env);
    assert!(output.is_empty(), "other tools pass without a word");
    assert_no_connection(&listener);
    Ok(())
}

/// SPEC-493 REV-002 (review round 1 of fix 1b): the Agent tool's `PreToolUse` goes to Yulius
/// with its whole input, and Yulius's answer (another reviewer as the tool's new input) is
/// printed for Claude Code. With no server it fails open: nothing is printed.
#[test]
fn a_subagents_start_goes_to_yulius_and_its_answer_is_printed() -> TestResult {
    let body = br#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow","updatedInput":{"subagent_type":"yulius-reviewer-opus","prompt":"p"}}}"#;
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    let (url, server) = serve_once(response)?;
    let payload = br#"{"hook_event_name":"PreToolUse","tool_name":"Agent","cwd":"/w","session_id":"s","tool_use_id":"toolu_1","tool_input":{"subagent_type":"yulius-reviewer","prompt":"p","description":"d","run_in_background":true}}"#;
    let mut output = Vec::new();
    let code = run_hook(
        "claude",
        "PreToolUse",
        &mut &payload[..],
        &mut output,
        &env_with(url, Some("task-token-SYNTHETIC")),
    );
    assert_eq!(code, 0);
    assert_eq!(output, body);
    let request = join(server)?;
    assert_eq!(
        request.line.trim_end(),
        "POST /hooks/claude/PreToolUse HTTP/1.1"
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&request.body)?,
        serde_json::json!({"hook_event_name": "PreToolUse", "tool_name": "Agent", "cwd": "/w",
            "tool_input": {"subagent_type": "yulius-reviewer", "prompt": "p", "description": "d",
                "run_in_background": true}})
    );
    let (gone, _refusing) = refused_url()?;
    let mut output = Vec::new();
    let code = run_hook(
        "claude",
        "PreToolUse",
        &mut &payload[..],
        &mut output,
        &env_with(gone, Some("t")),
    );
    assert_eq!((code, output.is_empty()), (0, true));
    Ok(())
}

/// SPEC-426 EDG-001 (review round 1): an edit's `PreToolUse` fails open like every hook (no
/// server, a 500) and is never held: a `202` prints nothing and is not asked again. A `Write`
/// larger than the forwarding bound still reaches the gate, trimmed.
#[test]
fn an_edit_check_fails_open_and_is_never_held() -> TestResult {
    let edit = br#"{"tool_name":"Write","tool_input":{"file_path":"/w/api.rs","content":"x"}}"#;
    let (gone, _refusing) = refused_url()?;
    let mut output = Vec::new();
    let code = run_hook(
        "claude",
        "PreToolUse",
        &mut &edit[..],
        &mut output,
        &env_with(gone, Some("t")),
    );
    assert_eq!((code, output.is_empty()), (0, true));

    for response in [
        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n".to_vec(),
        b"HTTP/1.1 202 Accepted\r\nRetry-After: 1\r\nContent-Length: 0\r\n\r\n".to_vec(),
    ] {
        let (url, server) = serve_once_within(response, Duration::from_secs(10))?;
        let mut output = Vec::new();
        let started = Instant::now();
        let code = run_hook(
            "claude",
            "PreToolUse",
            &mut &edit[..],
            &mut output,
            &env_with(url, Some("t")),
        );
        assert_eq!((code, output.is_empty()), (0, true));
        assert!(
            started.elapsed() < Duration::from_millis(900),
            "not asked again"
        );
        join(server)?;
    }

    let content = "y".repeat(MAX_INPUT_BYTES + 10);
    let big = serde_json::to_vec(&serde_json::json!({
        "tool_name": "Write", "tool_input": {"file_path": "/w/api.rs", "content": content}
    }))?;
    let (url, server) = serve_once_within(
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec(),
        Duration::from_secs(10),
    )?;
    let mut output = Vec::new();
    run_hook(
        "claude",
        "PreToolUse",
        &mut big.as_slice(),
        &mut output,
        &env_with(url, Some("t")),
    );
    let request = join(server).map_err(|e| format!("the large Write was not sent: {e}"))?;
    assert!(
        request.body.len() < 200,
        "trimmed: {} bytes",
        request.body.len()
    );
    let sent: serde_json::Value = serde_json::from_slice(&request.body)?;
    assert_eq!(sent["tool_input"]["file_path"], "/w/api.rs");

    // Non-string fields are left out, and an edit without a path is still asked about.
    let (url, server) = serve_once_within(
        b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_vec(),
        Duration::from_secs(10),
    )?;
    let odd = br#"{"tool_name":"Edit","cwd":7,"tool_input":{"file_path":["x"]}}"#;
    run_hook(
        "claude",
        "PreToolUse",
        &mut &odd[..],
        &mut Vec::new(),
        &env_with(url, Some("t")),
    );
    let request = join(server)?;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&request.body)?,
        serde_json::json!({"tool_name": "Edit"})
    );
    Ok(())
}

fn ok_response(body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

/// A conversation's transcripts in `dir`, as agy 1.5.10 lays them out: a step of each type in
/// `steps` (a `USER_INPUT` typed by a person, the others the model's or the system's), in the
/// compact `transcript.jsonl` and the full `transcript_full.jsonl`; the full one's path, as its
/// hooks name it.
fn agy_transcript(dir: &std::path::Path, steps: &[&str]) -> io::Result<String> {
    let mut text = String::new();
    for (index, kind) in steps.iter().enumerate() {
        let source = match *kind {
            "USER_INPUT" => "USER_EXPLICIT",
            "PLANNER_RESPONSE" => "MODEL",
            _ => "SYSTEM",
        };
        text.push_str(
            &serde_json::json!({
                "step_index": index, "source": source, "type": kind, "status": "DONE",
                "created_at": "2026-11-17T17:09:20Z", "content": "x"
            })
            .to_string(),
        );
        text.push('\n');
    }
    std::fs::write(dir.join("transcript.jsonl"), &text)?;
    let full = dir.join("transcript_full.jsonl");
    std::fs::write(&full, &text)?;
    Ok(full.to_string_lossy().into_owned())
}

/// The common fields of an Antigravity hook payload, as agy 1.5.10 sends them (synthetic values).
fn agy_payload(event: &str, extra: serde_json::Value) -> Vec<u8> {
    let mut payload = serde_json::json!({
        "artifactDirectoryPath": "/h/.gemini/antigravity-cli/brain/c-1",
        "conversationId": "c-1",
        "cwd": "/w/t1",
        "hookEventName": event,
        "lastUserInput": "Fix the failing test",
        "modelName": "gemini-4-carbon-high",
        "transcriptPath": "/h/.gemini/antigravity-cli/brain/c-1/.system_generated/logs/transcript_full.jsonl",
        "workspacePaths": ["/w/t1"],
    });
    if let (Some(base), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
        base.extend(extra.clone());
    }
    payload.to_string().into_bytes()
}

fn json_out(output: &[u8]) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::from_slice(output)
}

/// SPEC-505 AGY-006: an Antigravity SessionStart reaches Yulius as Claude Code's (its
/// conversation as the session, the transcript and folder), and the briefing comes back as a
/// system message the CLI injects.
#[test]
fn an_antigravity_session_start_is_sent_as_claude_codes() -> TestResult {
    let (url, server) = serve_once(ok_response(
        br#"{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"the briefing"}}"#,
    ))?;
    let mut output = Vec::new();
    let payload = agy_payload("SessionStart", serde_json::json!({}));
    run_hook(
        "antigravity",
        "SessionStart",
        &mut &payload[..],
        &mut output,
        &env_with(url, Some("task-token-SYNTHETIC")),
    );
    assert_eq!(
        json_out(&output)?,
        serde_json::json!({ "injectSteps": [{ "systemMessage": { "systemMessage": "the briefing" } }] })
    );
    let request = join(server)?;
    assert_eq!(
        request.line.trim_end(),
        "POST /hooks/antigravity/SessionStart HTTP/1.1"
    );
    let sent: serde_json::Value = serde_json::from_slice(&request.body)?;
    assert_eq!(sent["session_id"], "c-1");
    assert_eq!(sent["hook_event_name"], "SessionStart");
    assert_eq!(sent["source"], "startup");
    assert_eq!(sent["cwd"], "/w/t1");
    assert_eq!(sent["model"], "gemini-4-carbon-high");
    assert_eq!(
        sent["transcript_path"],
        "/h/.gemini/antigravity-cli/brain/c-1/.system_generated/logs/transcript.jsonl",
        "the transcript Yulius reads, not the full one"
    );
    Ok(())
}

/// SPEC-505 AGY-006: the first model call of a turn is its prompt (Claude Code's
/// UserPromptSubmit); a refusal ends the turn with the reason shown; a later call of the same
/// turn asks nothing.
#[test]
fn an_antigravity_turn_opens_with_its_prompt() -> TestResult {
    let dir = tempfile::tempdir()?;
    let transcript = agy_transcript(
        dir.path(),
        &["USER_INPUT", "PLANNER_RESPONSE", "USER_INPUT"],
    )?;
    let (url, server) = serve_once(ok_response(br#"{"decision":"block","reason":"Paused"}"#))?;
    let mut output = Vec::new();
    let payload = agy_payload(
        "PreInvocation",
        serde_json::json!({"invocationNum": 0, "initialNumSteps": 3, "transcriptPath": transcript}),
    );
    run_hook(
        "antigravity",
        "PreInvocation",
        &mut &payload[..],
        &mut output,
        &env_with(url, Some("task-token-SYNTHETIC")),
    );
    assert_eq!(
        json_out(&output)?,
        serde_json::json!({
            "injectSteps": [{ "errorMessage": { "userMessage": "Paused" } }],
            "terminationBehavior": "terminate"
        })
    );
    let request = join(server)?;
    assert_eq!(
        request.line.trim_end(),
        "POST /hooks/antigravity/UserPromptSubmit HTTP/1.1"
    );
    let sent: serde_json::Value = serde_json::from_slice(&request.body)?;
    assert_eq!(sent["prompt"], "Fix the failing test");
    assert_eq!(sent["hook_event_name"], "UserPromptSubmit");

    let (url, listener) = untouched_listener()?;
    let mut output = Vec::new();
    let later = agy_payload("PreInvocation", serde_json::json!({"invocationNum": 2}));
    run_hook(
        "antigravity",
        "PreInvocation",
        &mut &later[..],
        &mut output,
        &env_with(url, Some("task-token-SYNTHETIC")),
    );
    assert_eq!(json_out(&output)?, serde_json::json!({}));
    assert_no_connection(&listener);
    Ok(())
}

/// SPEC-505 AGY-006: a Stop Yulius blocks keeps the agent going with the reason; one it lets
/// pass prints an empty answer; the continuation flag is Claude Code's `stop_hook_active`.
#[test]
fn an_antigravity_stop_is_continued_or_let_pass() -> TestResult {
    let (url, server) = serve_once(ok_response(
        br#"{"decision":"block","reason":"Run the gate"}"#,
    ))?;
    let mut output = Vec::new();
    let payload = agy_payload(
        "Stop",
        serde_json::json!({"terminationReason": "NO_TOOL_CALL", "continuationActive": true, "fullyIdle": true}),
    );
    run_hook(
        "antigravity",
        "Stop",
        &mut &payload[..],
        &mut output,
        &env_with(url, Some("task-token-SYNTHETIC")),
    );
    assert_eq!(
        json_out(&output)?,
        serde_json::json!({ "decision": "continue", "reason": "Run the gate" })
    );
    let sent: serde_json::Value = serde_json::from_slice(&join(server)?.body)?;
    assert_eq!(sent["fully_idle"], true);
    assert_eq!(sent["stop_hook_active"], true);
    assert_eq!(sent["hook_event_name"], "Stop");

    let (url, server) = serve_once(ok_response(b""))?;
    let mut output = Vec::new();
    let payload = agy_payload("Stop", serde_json::json!({"fullyIdle": false}));
    run_hook(
        "antigravity",
        "Stop",
        &mut &payload[..],
        &mut output,
        &env_with(url, Some("task-token-SYNTHETIC")),
    );
    assert_eq!(json_out(&output)?, serde_json::json!({}));
    let sent: serde_json::Value = serde_json::from_slice(&join(server)?.body)?;
    assert_eq!(sent["fully_idle"], false);
    Ok(())
}

/// SPEC-505 AGY-006: agy denies a tool on an empty answer, so a `run_command` is allowed unless
/// Yulius's guard refuses it (checked in the sandbox, as Claude Code's `Bash`), and a Yulius that
/// cannot be reached still lets the agent work.
#[test]
fn an_antigravity_command_is_allowed_unless_the_guard_refuses_it() -> TestResult {
    let (url, listener) = untouched_listener()?;
    let env = env_with(url, Some("task-token-SYNTHETIC"));
    let tool = |command: &str| {
        agy_payload(
            "PreToolUse",
            serde_json::json!({"stepIdx": 4, "toolCall": {"id": "t1", "name": "run_command", "args": {"CommandLine": command, "toolSummary": "x"}}}),
        )
    };
    let mut output = Vec::new();
    run_hook(
        "antigravity",
        "PreToolUse",
        &mut &tool("cargo test")[..],
        &mut output,
        &env,
    );
    assert_eq!(
        json_out(&output)?,
        serde_json::json!({ "decision": "allow" })
    );
    let mut output = Vec::new();
    run_hook(
        "antigravity",
        "PreToolUse",
        &mut &tool("pkill -f cargo-nextest")[..],
        &mut output,
        &env,
    );
    let answer = json_out(&output)?;
    assert_eq!(answer["decision"], "deny");
    assert!(
        answer["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("pkill -f"),
        "{answer}"
    );
    assert_no_connection(&listener);
    // Broken input, or a tool the guard does not read: allowed.
    for body in [
        &b"{broken"[..],
        &agy_payload(
            "PreToolUse",
            serde_json::json!({"toolCall": {"name": "view_file", "args": {}}}),
        )[..],
    ] {
        let mut output = Vec::new();
        run_hook(
            "antigravity",
            "PreToolUse",
            &mut &body[..],
            &mut output,
            &env,
        );
        assert_eq!(
            json_out(&output)?,
            serde_json::json!({ "decision": "allow" })
        );
    }
    // Other events Yulius cannot answer print an empty answer the CLI reads as "go on".
    let mut output = Vec::new();
    let unreachable = env_with("http://127.0.0.1:9".into(), Some("task-token-SYNTHETIC"));
    let stop = agy_payload("Stop", serde_json::json!({}));
    run_hook_with(
        "antigravity",
        "Stop",
        &mut &stop[..],
        &mut output,
        &unreachable,
        HoldPolicy::NEVER,
    );
    assert_eq!(json_out(&output)?, serde_json::json!({}));
    Ok(())
}

/// SPEC-505 AGY-006: agy wakes itself when a background command ends, with a model call that
/// opens a turn but no typed prompt (the step before it is a system message, and
/// `lastUserInput` is still the earlier prompt). It reports work without a prompt; a missing
/// transcript does not identify a wake-up.
#[test]
fn an_antigravity_wake_up_is_no_prompt() -> TestResult {
    let dir = tempfile::tempdir()?;
    let transcript = agy_transcript(
        dir.path(),
        &[
            "USER_INPUT",
            "PLANNER_RESPONSE",
            "GENERIC",
            "SYSTEM_MESSAGE",
        ],
    )?;
    let (url, server) = serve_once_within(ok_response(b"{}"), Duration::from_secs(2))?;
    let mut output = Vec::new();
    let payload = agy_payload(
        "PreInvocation",
        serde_json::json!({"invocationNum": 0, "initialNumSteps": 4, "transcriptPath": transcript}),
    );
    run_hook(
        "antigravity",
        "PreInvocation",
        &mut &payload[..],
        &mut output,
        &env_with(url, Some("task-token-SYNTHETIC")),
    );
    assert_eq!(json_out(&output)?, serde_json::json!({}));
    let request = join(server)?;
    assert_eq!(
        request.line.trim_end(),
        "POST /hooks/antigravity/PreInvocation HTTP/1.1"
    );
    let sent: serde_json::Value = serde_json::from_slice(&request.body)?;
    assert!(sent.get("prompt").is_none());

    let (url, listener) = untouched_listener()?;
    let env = env_with(url, Some("task-token-SYNTHETIC"));
    let missing = dir.path().join("missing.jsonl");
    for (invocation, steps, path) in [
        (0, 4, missing.as_path()),
        (1, 4, std::path::Path::new(&transcript)),
        (0, 3, std::path::Path::new(&transcript)),
    ] {
        let payload = agy_payload(
            "PreInvocation",
            serde_json::json!({"invocationNum": invocation, "initialNumSteps": steps, "transcriptPath": path}),
        );
        let mut output = Vec::new();
        run_hook(
            "antigravity",
            "PreInvocation",
            &mut &payload[..],
            &mut output,
            &env,
        );
        assert_eq!(json_out(&output)?, serde_json::json!({}));
        assert_no_connection(&listener);
    }
    Ok(())
}

/// SPEC-505 AGY-006: a turn the model API ended with an error (agy's Stop with
/// `terminationReason` `ERROR`, after its retries) reaches Yulius as the turn's end with its
/// error, and no answer continues it: agy would only try the failing model again.
#[test]
fn an_antigravity_turn_ended_by_an_error_is_never_continued() -> TestResult {
    let (url, server) = serve_once(ok_response(
        br#"{"decision":"block","reason":"Run the gate"}"#,
    ))?;
    let mut output = Vec::new();
    let payload = agy_payload(
        "Stop",
        serde_json::json!({"terminationReason": "ERROR", "error": "Error 503 UNAVAILABLE", "continuationActive": false}),
    );
    run_hook(
        "antigravity",
        "Stop",
        &mut &payload[..],
        &mut output,
        &env_with(url, Some("task-token-SYNTHETIC")),
    );
    assert_eq!(json_out(&output)?, serde_json::json!({}));
    let request = join(server)?;
    assert_eq!(
        request.line.trim_end(),
        "POST /hooks/antigravity/Stop HTTP/1.1"
    );
    let sent: serde_json::Value = serde_json::from_slice(&request.body)?;
    assert_eq!(sent["error"], "Error 503 UNAVAILABLE");
    Ok(())
}

/// SPEC-505 AGY-006: a prompt Yulius refused ends agy's turn with a Stop of its own
/// (`TERMINAL_CUSTOM_HOOK`); Yulius is not asked about it, so nothing continues the refused
/// prompt (Claude Code runs no turn for a refused prompt).
#[test]
fn a_refused_antigravity_prompt_is_never_continued() -> TestResult {
    let (url, listener) = untouched_listener()?;
    let mut output = Vec::new();
    let payload = agy_payload(
        "Stop",
        serde_json::json!({"terminationReason": "TERMINAL_CUSTOM_HOOK", "continuationActive": false}),
    );
    run_hook(
        "antigravity",
        "Stop",
        &mut &payload[..],
        &mut output,
        &env_with(url, Some("task-token-SYNTHETIC")),
    );
    assert_eq!(json_out(&output)?, serde_json::json!({}));
    assert_no_connection(&listener);
    Ok(())
}

#[test]
fn claude_scratch_removal_permission_returns_a_checked_retry() -> TestResult {
    use serde_json::{Value, json};

    let answer = |payload: &[u8]| {
        let mut out = Vec::new();
        assert_eq!(
            run_hook(
                "claude",
                "PermissionRequest",
                &mut &payload[..],
                &mut out,
                &|_| None,
            ),
            0
        );
        out
    };
    let request = |command: &str| {
        json!({
            "hook_event_name": "PermissionRequest", "permission_mode": "bypassPermissions",
            "permission_suggestions": [], "tool_name": "Bash", "tool_input": {"command": command}
        })
    };
    for (command, retry) in [
        (
            r#"W=/work
S=/tmp/scratchpad
cd $W && PYTHONDONTWRITEBYTECODE=1 python3 -m unittest scripts/test_cd.py 2>&1 | tail -n 4
for d in r3m r3p; do
    rm -rf $S/$d
    mkdir -p $S/$d/scripts
    cp -r $W/scripts/cd $S/$d/scripts/cd
done
cd $S/r3m && n=$(grep -n '^  phase=installed$' scripts/cd/remote.sh | sed -n 2p | cut -d: -f1)
sed -i "${n}s/.* /   : # mutated/" scripts/cd/remote.sh && sed -n "$((n-2)),$((n+1))p" scripts/cd/remote.sh
python3 -m unittest scripts/test_cd.py 2>&1 | grep -E "^(FAIL|Ran|OK)"
cd $S/probe && python3 r2.py $S/r3p/scripts 2>&1 | tail -n 8"#,
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "${S:?}/${d:?}""#,
        ),
        (
            r#"rm -fr -- "${scratch}/${copy}""#,
            r#"yulius remove --preserve-trailing-slash -- "${scratch:?}/${copy:?}""#,
        ),
        (
            r#"rm -rf "$scratch""#,
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "${scratch:?}""#,
        ),
        (
            r#"rm -rf "$scratch" && echo rebuilt &>result"#,
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "${scratch:?}""#,
        ),
        (
            r#"rm -rf "$scratch" || echo failed &>>result"#,
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "${scratch:?}""#,
        ),
        (
            r#"{ rm -rf "$scratch"; } >result"#,
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "${scratch:?}""#,
        ),
        (
            r#"rm -rf "$scratch" && echo rebuilt &>result # <<"#,
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "${scratch:?}""#,
        ),
        (
            r#"rm -rf "$scratch" && echo "<< rebuilt" &>result"#,
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "${scratch:?}""#,
        ),
        (
            "cat <<'END&>'\nEND>\nrm -rf $data\nEND&>\nrm -rf \"$scratch\" && echo rebuilt &>result",
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "${scratch:?}""#,
        ),
        (
            r#"rm -rf "$TMPDIR/scratch""#,
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "${TMPDIR:?}/scratch""#,
        ),
        (
            r#"rm "$scratch/""#,
            r#"yulius remove --preserve-trailing-slash --reject-option-operands --file-only --require-existing -- "${scratch:?}/""#,
        ),
        (
            "rm -rf $S/leaf",
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "${S:?}/leaf""#,
        ),
        (
            r#"rm() { return 0; }; /bin/rm -rf "$scratch""#,
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "${scratch:?}""#,
        ),
        (
            "rm -rf $S/$d another-path",
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "${S:?}/${d:?}" "another-path""#,
        ),
        (
            "if test -d scratch; then rm -rf $S/$d; fi",
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "${S:?}/${d:?}""#,
        ),
        (
            r#"test -f marker && rm -f "$S/$d""#,
            r#"yulius remove --preserve-trailing-slash --reject-option-operands --file-only -- "${S:?}/${d:?}""#,
        ),
        (
            "rm -f /task.log /probe.log",
            r#"yulius remove --preserve-trailing-slash --reject-option-operands --file-only -- "/task.log" "/probe.log""#,
        ),
        (
            "rm /task.log",
            r#"yulius remove --preserve-trailing-slash --reject-option-operands --file-only --require-existing -- "/task.log""#,
        ),
        (
            "rm -r /scratch",
            r#"yulius remove --preserve-trailing-slash --reject-option-operands --require-existing -- "/scratch""#,
        ),
        (
            "rm -rf '/scratch dir'",
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "/scratch dir""#,
        ),
        (
            "rm -rf /scratch/",
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "/scratch/""#,
        ),
        (
            "rm -rf //scratch///",
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "//scratch///""#,
        ),
        (
            "while test -e ready; do rm --recursive --force /artifact; done",
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "/artifact""#,
        ),
        (
            "until test -e ready; do rm -R -f /artifact; done",
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "/artifact""#,
        ),
        (
            "case x in x) (rm -f /artifact);; esac",
            r#"yulius remove --preserve-trailing-slash --reject-option-operands --file-only -- "/artifact""#,
        ),
        (
            "true || { rm -f /artifact; }",
            r#"yulius remove --preserve-trailing-slash --reject-option-operands --file-only -- "/artifact""#,
        ),
        (
            "if false; then true; else rm -f /artifact | cat; fi",
            r#"yulius remove --preserve-trailing-slash --reject-option-operands --file-only -- "/artifact""#,
        ),
        (
            "rm -rf -- -name /artifact",
            r#"yulius remove --preserve-trailing-slash -- "-name" "/artifact""#,
        ),
        (
            r#"rm -rf "${S}/some space""#,
            r#"yulius remove --preserve-trailing-slash --reject-option-operands -- "${S:?}/some space""#,
        ),
    ] {
        let mut event = request(command);
        for worker in [false, true] {
            if worker {
                event["agent_id"] = json!("reviewer-fixture");
            }
            let reply: Value = serde_json::from_slice(&answer(&serde_json::to_vec(&event)?))?;
            let output = &reply["hookSpecificOutput"];
            assert_eq!(output["hookEventName"], "PermissionRequest");
            let decision = &output["decision"];
            assert_eq!(decision["behavior"], "deny");
            assert_eq!(decision["interrupt"], false);
            let message = decision["message"]
                .as_str()
                .ok_or("missing retry message")?;
            assert!(message.contains(retry), "{message}");
            assert!(
                message.contains("in place")
                    && message.contains("conditions")
                    && message.contains("variable bindings"),
                "{message}"
            );
        }
    }
    for command in [
        r#"rm -rf "${S:?}/${d:?}""#,
        "rm -rf /tmp/scratch",
        r#"rm -rf "$S/"'$(echo unintended)'"#,
        r#"rm -rf "$S/"'`echo unintended`'"#,
        r#"rm -rf "$scratch$""#,
        r#"rm -rf "$10""#,
        "rm -rf $10",
        r#"rm -rf "${10}""#,
        "rm -rf $S $(echo x)",
        "rm -rf $S -i",
        "rm --interactive=once $S/$d",
        r#"rm <required-marker -rf "$scratch""#,
        r#"<required-marker rm -rf "$scratch""#,
        r#"rm -rf "$scratch" 2>result"#,
        r#"FLAG=1 rm -rf "$scratch""#,
        r#"rm -rf "$scratch" &>result"#,
        r#"rm -rf "$scratch" &>>result"#,
        r#"true && rm -rf "$scratch" &>result"#,
        r#"echo ready | rm -rf "$scratch" 2>result"#,
        r#"rm -rf "$scratch" &"#,
        r#"rm -rf "$scratch" && echo rebuilt & >result"#,
        r#"rm -rf "$scratch" && echo rebuilt &"#,
        "cat <<'END&>'\nEND>\nrm -rf $scratch\nEND&>",
        r#"rm() { return 0; }; rm -rf "$scratch""#,
        r#"if true; then function rm { echo dry-run; }; fi; rm -rf "$scratch""#,
        "rm - $S/$d",
        "rm -rf",
        "rm -rf '/a/b/c'",
        "rm -i $S/$d",
        "curl https://example.invalid",
        "echo do rm -rf $S/$d",
        "printf '%s' 'rm -rf $S/$d'",
        "rm -rf '$S/$d'",
        "cat <<'END'\nrm -rf $S/$d\nEND",
        "echo ok # rm -rf $S/$d",
        "rm -rf $S/$d &&",
    ] {
        assert!(
            answer(&serde_json::to_vec(&request(command))?).is_empty(),
            "{command}"
        );
    }
    for (key, value) in [
        ("tool_name", json!("Read")),
        ("permission_mode", json!("plan")),
        ("hook_event_name", json!("PreToolUse")),
        ("tool_input", json!(null)),
        ("permission_suggestions", json!([{"type":"addRules"}])),
    ] {
        let mut event = request("rm -rf $S/$d");
        event[key] = value;
        assert!(answer(&serde_json::to_vec(&event)?).is_empty(), "{key}");
    }
    let payload = serde_json::to_vec(&request("rm -rf $scratch"))?;
    for (harness, event) in [("codex", "PermissionRequest"), ("claude", "SessionStart")] {
        let mut out = Vec::new();
        assert_eq!(
            run_hook(harness, event, &mut payload.as_slice(), &mut out, &|_| None),
            0
        );
        assert!(out.is_empty(), "{harness} {event}");
    }
    let reply: Value =
        serde_json::from_slice(&answer(&serde_json::to_vec(&request("rm -rf $scratch"))?))?;
    let retry = reply["hookSpecificOutput"]["decision"]["message"]
        .as_str()
        .and_then(|message| message.split('`').nth(1))
        .ok_or("missing complete retry")?;
    for (setup, external) in [("", true), ("rm() { :; }", false), ("alias rm=true", false)] {
        let run = std::process::Command::new("bash")
            .env_clear().env("PATH", "/usr/bin:/bin")
            .args(["-c", r#"shopt -s expand_aliases; eval "$1"; scratch=fixture; yulius() { printf called; }; eval "$2""#, "fixture", setup, retry])
            .output()?;
        assert_eq!(run.status.success(), external, "{setup}");
        assert_eq!(run.stdout == b"called", external, "{setup}");
    }
    let mut at_limit = serde_json::to_vec(&request("rm -rf $S/$d"))?;
    at_limit.resize(MAX_INPUT_BYTES, b' ');
    assert!(!answer(&at_limit).is_empty());
    assert!(answer(b"invalid json").is_empty());
    let mut oversized = serde_json::to_vec(&request("rm -rf $S/$d"))?;
    oversized.resize(MAX_INPUT_BYTES + 1, b' ');
    assert!(answer(&oversized).is_empty());
    Ok(())
}

/// SPEC-652 MEM-006 (review of part 2): `yulius hook <claude|codex> MemoryGuard` is answered in the
/// sandbox, without Yulius (no `YULIUS_URL`), from the run's read-only paths; an Antigravity
/// session with them hears the sentence at its start even when Yulius cannot be reached.
#[test]
fn the_memory_guard_answers_in_the_sandbox() {
    let sentence = yulius_contracts::hooks::MEMORY_READ_ONLY_SENTENCE;
    let env = |name: &str| {
        (name == "YULIUS_READONLY_PATHS").then(|| "/s/home/.claude/projects/-k/memory".to_owned())
    };
    let write = serde_json::json!({
        "hook_event_name": "PreToolUse", "tool_name": "Write",
        "tool_input": { "file_path": "/s/home/.claude/projects/-k/memory/x.md" },
    })
    .to_string();
    for harness in ["claude", "codex"] {
        let mut out = Vec::new();
        let code = run_hook_with(
            harness,
            "MemoryGuard",
            &mut write.as_bytes(),
            &mut out,
            &env,
            HoldPolicy::NEVER,
        );
        assert_eq!(code, 0);
        let reply: serde_json::Value = serde_json::from_slice(&out).unwrap_or_default();
        assert_eq!(
            reply["hookSpecificOutput"]["permissionDecisionReason"], sentence,
            "{harness}"
        );
    }
    let start = serde_json::json!({ "conversationId": "c1", "cwd": "/s/worktrees/t1" }).to_string();
    let mut out = Vec::new();
    run_hook_with(
        "antigravity",
        "SessionStart",
        &mut start.as_bytes(),
        &mut out,
        &env,
        HoldPolicy::NEVER,
    );
    let reply: serde_json::Value = serde_json::from_slice(&out).unwrap_or_default();
    assert_eq!(
        reply["injectSteps"][0]["systemMessage"]["systemMessage"], sentence,
        "{reply}"
    );
    let mut quiet = Vec::new();
    run_hook_with(
        "antigravity",
        "SessionStart",
        &mut start.as_bytes(),
        &mut quiet,
        &|_| None,
        HoldPolicy::NEVER,
    );
    assert!(!String::from_utf8_lossy(&quiet).contains(sentence));
}
