//! Connection test for configured MCP servers: JSON-RPC `initialize` followed by `tools/list`
//! over Streamable HTTP or stdio, with bounded time and output (SPEC-001 MCP-002).
//!
//! This is the only network and subprocess I/O of the MCP configuration area. Error texts never
//! contain secret values: URLs are stripped from HTTP client errors, arguments and environment
//! are never echoed, and server error messages are truncated.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{ChildStdin, ChildStdout};

/// Total budget of an HTTP test.
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
/// Budget of a stdio test before the process group is killed.
pub const STDIO_TIMEOUT: Duration = Duration::from_secs(10);
/// Wait for the killed stdio server to be reaped.
pub const REAP_TIMEOUT: Duration = Duration::from_secs(5);
/// Most bytes read from one HTTP response or from a stdio server in total.
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
/// Protocol version offered in `initialize`.
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Most `tools/list` pages followed.
const MAX_TOOL_PAGES: u64 = 10;
/// Most tool names returned.
const MAX_TOOLS: usize = 1_000;
/// Longest server-provided text kept in an error.
const MAX_ERROR_CHARS: usize = 300;

/// What to run for a stdio test.
#[derive(Debug, Clone, Copy)]
pub struct StdioSpec<'a> {
    /// Executable (resolved through `PATH`; never through a shell).
    pub command: &'a str,
    /// Arguments.
    pub args: &'a [String],
    /// Secret environment (names and values).
    pub env: &'a [(String, String)],
    /// `HOME` and working directory of the child.
    pub home: &'a Path,
}

/// The `initialize` request.
#[must_use]
pub fn initialize_request(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": "yulius", "version": env!("CARGO_PKG_VERSION")},
        },
    })
}

/// The `notifications/initialized` notification.
#[must_use]
pub fn initialized_notification() -> Value {
    json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
}

/// A `tools/list` request.
#[must_use]
pub fn tools_list_request(id: u64, cursor: Option<&str>) -> Value {
    match cursor {
        Some(cursor) => {
            json!({"jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {"cursor": cursor}})
        }
        None => json!({"jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {}}),
    }
}

fn truncate(text: &str) -> String {
    let mut out: String = text.chars().take(MAX_ERROR_CHARS).collect();
    if text.chars().count() > MAX_ERROR_CHARS {
        out.push('…');
    }
    out
}

fn matches_id(message: &Value, id: u64) -> bool {
    message.get("id").and_then(Value::as_u64) == Some(id)
        && (message.get("result").is_some() || message.get("error").is_some())
}

fn find_in_json(value: Value, id: u64) -> Option<Value> {
    match value {
        Value::Array(items) => items.into_iter().find(|m| matches_id(m, id)),
        other => matches_id(&other, id).then_some(other),
    }
}

/// Finds the JSON-RPC response with `id` in a response body: a JSON object, a JSON batch, or a
/// `text/event-stream` whose `data:` lines carry JSON messages.
#[must_use]
pub fn find_response(content_type: &str, body: &str, id: u64) -> Option<Value> {
    if content_type.starts_with("text/event-stream") {
        let mut data = String::new();
        for line in body.lines().chain(std::iter::once("")) {
            if let Some(rest) = line.strip_prefix("data:") {
                if !data.is_empty() {
                    data.push('\n');
                }
                data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            } else if line.trim().is_empty() && !data.is_empty() {
                if let Some(found) = serde_json::from_str(&data)
                    .ok()
                    .and_then(|v| find_in_json(v, id))
                {
                    return Some(found);
                }
                data.clear();
            }
        }
        None
    } else {
        serde_json::from_str(body)
            .ok()
            .and_then(|v| find_in_json(v, id))
    }
}

/// The `result` of a response, or the server's error message.
///
/// # Errors
/// The server's JSON-RPC error (truncated).
pub fn rpc_result(response: &Value) -> Result<&Value, String> {
    if let Some(error) = response.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(format!(
            "the server returned an error: {}",
            truncate(message)
        ));
    }
    response
        .get("result")
        .ok_or_else(|| "the server sent a response without a result".to_owned())
}

/// Tool names and the next cursor of a `tools/list` result.
///
/// # Errors
/// A result without a `tools` array.
pub fn tool_page(result: &Value) -> Result<(Vec<String>, Option<String>), String> {
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| "tools/list returned no tools array".to_owned())?;
    let names = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .map(|n| n.chars().take(200).collect())
        .collect();
    let cursor = result
        .get("nextCursor")
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty())
        .map(str::to_owned);
    Ok((names, cursor))
}

fn http_error(err: reqwest::Error) -> String {
    let err = err.without_url();
    if err.is_timeout() {
        "the server did not answer in time".to_owned()
    } else if err.is_connect() {
        format!("could not connect: {err}")
    } else {
        format!("request failed: {err}")
    }
}

struct HttpSession {
    client: reqwest::Client,
    url: String,
    headers: reqwest::header::HeaderMap,
    session: Option<String>,
    protocol: Option<String>,
}

impl HttpSession {
    async fn post(&mut self, message: &Value, id: Option<u64>) -> Result<Option<Value>, String> {
        let mut request = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .json(message);
        if let Some(session) = &self.session {
            request = request.header("mcp-session-id", session);
        }
        if let Some(protocol) = &self.protocol {
            request = request.header("mcp-protocol-version", protocol);
        }
        let mut response = request.send().await.map_err(http_error)?;
        let status = response.status();
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            self.session = Some(session.to_owned());
        }
        if !status.is_success() {
            return Err(format!("the server answered HTTP {status}"));
        }
        let Some(id) = id else {
            return Ok(None);
        };
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut body: Vec<u8> = Vec::new();
        // Read incrementally: an event stream may stay open after the response was sent.
        while let Some(chunk) = response.chunk().await.map_err(http_error)? {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_RESPONSE_BYTES {
                return Err("the server's response is larger than 1 MiB".to_owned());
            }
            if content_type.starts_with("text/event-stream")
                && let Some(found) =
                    find_response(&content_type, &String::from_utf8_lossy(&body), id)
            {
                return Ok(Some(found));
            }
        }
        find_response(&content_type, &String::from_utf8_lossy(&body), id)
            .map(Some)
            .ok_or_else(|| "the server's answer contained no JSON-RPC response".to_owned())
    }
}

/// Where a connection test may connect (SSRF guard, SPEC-002 R-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AddressClass {
    /// A public address: any write user may test it.
    Public,
    /// Loopback or private network: instance administrators only.
    Private,
    /// Link-local (cloud metadata), unspecified, multicast or broadcast: never.
    Forbidden,
}

/// Classifies an IP address for the connection test.
#[must_use]
pub fn classify(ip: std::net::IpAddr) -> AddressClass {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_link_local() || v4.is_unspecified() || v4.is_multicast() || v4.is_broadcast() {
                AddressClass::Forbidden
            } else if v4.is_loopback()
                || v4.is_private()
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64)
            {
                AddressClass::Private
            } else {
                AddressClass::Public
            }
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return classify(IpAddr::V4(mapped));
            }
            let first = v6.segments()[0];
            if v6.is_unspecified() || v6.is_multicast() || (first & 0xffc0) == 0xfe80 {
                AddressClass::Forbidden
            } else if v6.is_loopback() || (first & 0xfe00) == 0xfc00 {
                AddressClass::Private
            } else {
                AddressClass::Public
            }
        }
    }
}

/// Resolves the host of an `http`/`https` URL; returns the host name and its addresses.
///
/// # Errors
/// A human-readable reason for invalid URLs or failed lookups.
pub async fn resolve_target(url: &str) -> Result<(String, Vec<std::net::SocketAddr>), String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| "invalid server URL".to_owned())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("the server URL must use http or https".into());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "the server URL has no host".to_owned())?
        .trim_matches(['[', ']'])
        .to_owned();
    let port = parsed.port_or_known_default().unwrap_or(443);
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|_| format!("cannot resolve {host}"))?
        .take(16)
        .collect();
    if addrs.is_empty() {
        return Err(format!("cannot resolve {host}"));
    }
    Ok((host, addrs))
}

async fn http_tools(
    url: &str,
    headers: &[(String, String)],
    pinned: Option<(&str, &[std::net::SocketAddr])>,
) -> Result<Vec<String>, String> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut map = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("invalid header name {name}"))?;
        let mut value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|_| format!("invalid value for header {name}"))?;
        value.set_sensitive(true);
        map.insert(name, value);
    }
    let mut builder = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none());
    if let Some((host, addrs)) = pinned {
        // Connect only to the addresses that were checked (no DNS rebinding in between).
        builder = builder.resolve_to_addrs(host, addrs);
    }
    let client = builder.build().map_err(http_error)?;
    let mut session = HttpSession {
        client,
        url: url.to_owned(),
        headers: map,
        session: None,
        protocol: None,
    };
    let init = session
        .post(&initialize_request(1), Some(1))
        .await?
        .ok_or_else(|| "no initialize response".to_owned())?;
    let result = rpc_result(&init)?;
    session.protocol = Some(
        result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(PROTOCOL_VERSION)
            .to_owned(),
    );
    // Servers acknowledge the notification with 202; a failure here is not fatal.
    let _ = session.post(&initialized_notification(), None).await;
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for id in 2..2 + MAX_TOOL_PAGES {
        let response = session
            .post(&tools_list_request(id, cursor.as_deref()), Some(id))
            .await?
            .ok_or_else(|| "no tools/list response".to_owned())?;
        let (names, next) = tool_page(rpc_result(&response)?)?;
        tools.extend(names);
        cursor = next;
        if cursor.is_none() || tools.len() >= MAX_TOOLS {
            break;
        }
    }
    tools.truncate(MAX_TOOLS);
    Ok(tools)
}

/// Tests a Streamable HTTP server within [`HTTP_TIMEOUT`] (no redirects followed). With
/// `pinned`, connections go only to those addresses of that host.
///
/// # Errors
/// A redacted, human-readable reason.
pub async fn probe_http(
    url: &str,
    headers: &[(String, String)],
    pinned: Option<(&str, &[std::net::SocketAddr])>,
) -> Result<Vec<String>, String> {
    match tokio::time::timeout(HTTP_TIMEOUT, http_tools(url, headers, pinned)).await {
        Ok(result) => result,
        Err(_) => Err(format!(
            "the server did not answer within {} s",
            HTTP_TIMEOUT.as_secs()
        )),
    }
}

/// Newline-delimited JSON reader over a child's stdout with a total byte bound.
struct LineReader {
    stdout: ChildStdout,
    buffer: Vec<u8>,
    total: usize,
}

impl LineReader {
    async fn response(&mut self, id: u64) -> Result<Value, String> {
        loop {
            while let Some(pos) = self.buffer.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buffer.drain(..=pos).collect();
                if let Ok(message) = serde_json::from_slice::<Value>(&line)
                    && matches_id(&message, id)
                {
                    return Ok(message);
                }
            }
            let mut chunk = [0_u8; 8192];
            let read = self
                .stdout
                .read(&mut chunk)
                .await
                .map_err(|e| format!("reading the server's output failed: {}", e.kind()))?;
            if read == 0 {
                return Err("the server exited or closed its output before answering".to_owned());
            }
            self.total += read;
            if self.total > MAX_RESPONSE_BYTES {
                return Err("the server wrote more than 1 MiB".to_owned());
            }
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }
}

async fn send(stdin: &mut ChildStdin, message: &Value) -> Result<(), String> {
    let mut line = serde_json::to_vec(message).map_err(|e| e.to_string())?;
    line.push(b'\n');
    stdin
        .write_all(&line)
        .await
        .map_err(|e| format!("writing to the server failed: {}", e.kind()))?;
    stdin
        .flush()
        .await
        .map_err(|e| format!("writing to the server failed: {}", e.kind()))
}

async fn stdio_tools(mut stdin: ChildStdin, stdout: ChildStdout) -> Result<Vec<String>, String> {
    let mut reader = LineReader {
        stdout,
        buffer: Vec::new(),
        total: 0,
    };
    send(&mut stdin, &initialize_request(1)).await?;
    let init = reader.response(1).await?;
    rpc_result(&init)?;
    send(&mut stdin, &initialized_notification()).await?;
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for id in 2..2 + MAX_TOOL_PAGES {
        send(&mut stdin, &tools_list_request(id, cursor.as_deref())).await?;
        let response = reader.response(id).await?;
        let (names, next) = tool_page(rpc_result(&response)?)?;
        tools.extend(names);
        cursor = next;
        if cursor.is_none() || tools.len() >= MAX_TOOLS {
            break;
        }
    }
    tools.truncate(MAX_TOOLS);
    Ok(tools)
}

/// Tests a stdio server: spawns `command` with argv (no shell) in its own process group with a
/// minimal environment (`PATH`, locale, `HOME` = `spec.home`) plus its secrets, exchanges
/// `initialize`/`tools/list` within `timeout`, then kills the whole process group and reaps the
/// child within [`REAP_TIMEOUT`].
///
/// # Errors
/// A human-readable reason (never including arguments or environment values).
pub async fn probe_stdio(spec: &StdioSpec<'_>, timeout: Duration) -> Result<Vec<String>, String> {
    let mut command = tokio::process::Command::new(spec.command);
    command.args(spec.args).env_clear();
    for key in ["PATH", "LANG", "LC_ALL", "TMPDIR"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command
        .env("HOME", spec.home)
        .envs(spec.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .current_dir(spec.home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = command
        .spawn()
        .map_err(|e| format!("could not start {}: {}", spec.command, e.kind()))?;
    let pid = child.id().and_then(|p| i32::try_from(p).ok());
    let result = match (child.stdin.take(), child.stdout.take()) {
        (Some(stdin), Some(stdout)) => {
            match tokio::time::timeout(timeout, stdio_tools(stdin, stdout)).await {
                Ok(result) => result,
                Err(_) => Err(format!(
                    "the server did not answer within {} s",
                    timeout.as_secs()
                )),
            }
        }
        _ => Err("could not connect to the server's stdin/stdout".to_owned()),
    };
    // The child is not reaped yet, so its process group id cannot have been reused.
    if let Some(pid) = pid {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    let _ = child.start_kill();
    match tokio::time::timeout(REAP_TIMEOUT, child.wait()).await {
        Ok(_) => result,
        Err(_) => Err("the server process did not exit after being killed".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn classify_blocks_metadata_and_gates_private_networks() {
        use super::{AddressClass, classify};
        let ip = |text: &str| {
            text.parse::<std::net::IpAddr>()
                .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
        };
        assert_eq!(classify(ip("169.254.169.254")), AddressClass::Forbidden);
        assert_eq!(classify(ip("0.0.0.0")), AddressClass::Forbidden);
        assert_eq!(classify(ip("fe80::1")), AddressClass::Forbidden);
        assert_eq!(
            classify(ip("::ffff:169.254.169.254")),
            AddressClass::Forbidden
        );
        assert_eq!(classify(ip("127.0.0.1")), AddressClass::Private);
        assert_eq!(classify(ip("10.1.2.3")), AddressClass::Private);
        assert_eq!(classify(ip("192.168.1.1")), AddressClass::Private);
        assert_eq!(classify(ip("100.64.0.1")), AddressClass::Private);
        assert_eq!(classify(ip("::1")), AddressClass::Private);
        assert_eq!(classify(ip("fd00::1")), AddressClass::Private);
        assert_eq!(classify(ip("140.82.112.3")), AddressClass::Public);
        assert_eq!(classify(ip("2606:4700::1111")), AddressClass::Public);
    }

    use super::*;

    #[test]
    fn json_and_batch_bodies() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        assert!(find_response("application/json", body, 1).is_some());
        assert!(find_response("application/json", body, 2).is_none());
        let batch = r#"[{"jsonrpc":"2.0","method":"note"},{"jsonrpc":"2.0","id":3,"result":{}}]"#;
        assert!(find_response("application/json", batch, 3).is_some());
        assert!(find_response("application/json", "not json", 1).is_none());
    }

    #[test]
    fn event_stream_bodies() {
        // The second event's data spans two `data:` lines and the stream ends without a blank
        // line after it.
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"log\"}\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\ndata: \"result\":{\"tools\":[]}}\n";
        assert!(find_response("text/event-stream; charset=utf-8", body, 2).is_some());
        assert!(find_response("text/event-stream", body, 1).is_none());
    }

    #[test]
    fn results_errors_and_pages() -> Result<(), String> {
        let error =
            json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -1, "message": "x".repeat(1000)}});
        let message = rpc_result(&error).err().unwrap_or_default();
        assert!(message.len() < 400 && message.ends_with('…'));
        let page = json!({"tools": [{"name": "search"}, {"name": "fetch"}, {"title": "unnamed"}], "nextCursor": "c2"});
        let (names, next) = tool_page(&page)?;
        assert_eq!(names, ["search", "fetch"]);
        assert_eq!(next.as_deref(), Some("c2"));
        assert!(tool_page(&json!({})).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn stdio_server_answers() -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let script = concat!(
            "read a; ",
            "echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{}}}'; ",
            "read b; read c; ",
            "echo '{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\"}'; ",
            "echo \"{\\\"jsonrpc\\\":\\\"2.0\\\",\\\"id\\\":2,\\\"result\\\":{\\\"tools\\\":[{\\\"name\\\":\\\"$TOOL_NAME\\\"}]}}\"; ",
            "sleep 30"
        );
        let args = vec!["-c".to_owned(), script.to_owned()];
        let env = vec![("TOOL_NAME".to_owned(), "secret_tool".to_owned())];
        let tools = probe_stdio(
            &StdioSpec {
                command: "/bin/sh",
                args: &args,
                env: &env,
                home: home.path(),
            },
            Duration::from_secs(10),
        )
        .await?;
        assert_eq!(
            tools,
            ["secret_tool"],
            "secrets reach the child environment"
        );
        Ok(())
    }

    /// A silent server is killed with its whole process group after the timeout.
    #[tokio::test]
    async fn silent_stdio_server_times_out_and_is_reaped() -> Result<(), Box<dyn std::error::Error>>
    {
        let home = tempfile::tempdir()?;
        let marker = home.path().join("grandchild.pid");
        let script = format!("sleep 60 & echo $! > '{}'; wait", marker.display());
        let args = vec!["-c".to_owned(), script];
        let started = std::time::Instant::now();
        let result = probe_stdio(
            &StdioSpec {
                command: "/bin/sh",
                args: &args,
                env: &[],
                home: home.path(),
            },
            Duration::from_millis(500),
        )
        .await;
        assert!(
            matches!(&result, Err(e) if e.contains("did not answer")),
            "{result:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        let pid: i32 = std::fs::read_to_string(&marker)?.trim().parse()?;
        // The background grandchild was in the same process group and received SIGKILL.
        let mut alive = true;
        for _ in 0..50 {
            alive = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok();
            if !alive {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!alive, "grandchild {pid} survived");
        Ok(())
    }

    #[tokio::test]
    async fn missing_command_is_reported() -> Result<(), Box<dyn std::error::Error>> {
        let home = tempfile::tempdir()?;
        let result = probe_stdio(
            &StdioSpec {
                command: "/nonexistent/yulius-mcp-server",
                args: &[],
                env: &[],
                home: home.path(),
            },
            Duration::from_secs(1),
        )
        .await;
        assert!(matches!(&result, Err(e) if e.starts_with("could not start")));
        Ok(())
    }
}
