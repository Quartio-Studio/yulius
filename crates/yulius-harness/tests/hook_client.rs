//! Hook client contracts against an in-test loopback HTTP server: forwarding, fail-open
//! behavior and bounds (SPEC-002 §7.5, §9 hook call 5 s).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread::{self, JoinHandle};

use yulius_harness::hook_client::{MAX_INPUT_BYTES, run_hook};

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
    Ok(())
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
