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
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let url = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    drop(listener);
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
    let payload = br#"{"hook_event_name":"Stop"}"#;
    let code = run_hook_with(
        "claude",
        "Stop",
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
    let (url, listener) = untouched_listener()?;
    drop(listener);
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
