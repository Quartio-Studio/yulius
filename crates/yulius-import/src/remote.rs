//! Client of a Yulius instance's remote MCP (SPEC-247 RMT-030, RMT-031): `yulius remote` and its
//! shortcuts call the same tools a person's local agent calls, with the same personal API token,
//! over `POST /mcp` (JSON-RPC, JSON answers).

use std::io::Read;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::Url;
use reqwest::blocking::Client;
use serde_json::{Value, json};

use crate::http::{ApiError, install_crypto_provider, validate_server_url};
use crate::secret::Secret;

/// Answers larger than this are refused (a tool's text is at most 256 KiB, its record as much).
const MAX_ANSWER_BYTES: u64 = 2 * 1024 * 1024;
/// One request's time limit: a tool such as `project_import` clones a repository.
const TIMEOUT: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// The MCP protocol version the client speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// The header a moved instance's old URL answers with, beside its redirect (SPEC-460 MIG-041).
const MOVED_HEADER: &str = "x-yulius-moved-to";

/// The base URL an instance moved to, when the answer to a request to `endpoint` is its old
/// host's redirect (SPEC-466 MVC-030, MVC-031): a redirect whose `X-Yulius-Moved-To` names a bare
/// origin (`https`, or plain `http` on loopback when this one is such a development server too)
/// and whose `Location` is the request's own path at that origin. Any other redirect is `None`:
/// the token is never sent where a response merely points.
fn moved_base(
    endpoint: &Url,
    status: StatusCode,
    moved: Option<&str>,
    location: Option<&str>,
) -> Option<Url> {
    if !status.is_redirection() {
        return None;
    }
    let origin = Url::parse(moved?.trim()).ok()?;
    let bare = origin.path() == "/"
        && origin.query().is_none()
        && origin.fragment().is_none()
        && origin.username().is_empty()
        && origin.password().is_none();
    let secure = origin.scheme() == "https" || endpoint.scheme() == "http";
    if !bare || !secure || validate_server_url(&origin).is_err() {
        return None;
    }
    let mut there = endpoint.clone();
    there.set_scheme(origin.scheme()).ok()?;
    there.set_host(origin.host_str()).ok()?;
    there.set_port(origin.port()).ok()?;
    // A redirect to the URL itself moves nothing.
    if there == *endpoint || Url::parse(location?.trim()).ok()? != there {
        return None;
    }
    // The endpoint is the base URL with `mcp` appended.
    there.path_segments_mut().ok()?.pop();
    Some(there)
}

/// A failed remote request.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    /// The server URL is unusable.
    #[error(transparent)]
    Url(#[from] ApiError),
    /// The instance moved to this base URL (its old host says so, SPEC-466 MVC-030). Nothing was
    /// sent there: the caller moves the saved sign-in and asks again, once.
    #[error("the instance moved to {to}: run `yulius remote login {to}`", to = .0.as_str().trim_end_matches('/'))]
    Moved(Url),
    /// `401`: no such token, or it was revoked.
    #[error(
        "the token was refused: it is wrong or revoked; make one on Settings → Connect your local agent and run `yulius remote login`"
    )]
    Unauthenticated,
    /// `403`: the token may not use the remote MCP (a `yulius login` token).
    #[error("{0}")]
    Forbidden(String),
    /// `429`: too many requests with this token.
    #[error("{0}")]
    RateLimited(String),
    /// Transport failures and other statuses.
    #[error("{0}")]
    Transport(String),
    /// The answer is not what the protocol says.
    #[error("unexpected answer from the server: {0}")]
    Protocol(String),
}

/// What a tool answered.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutcome {
    /// The tool failed (its text says why).
    pub is_error: bool,
    /// The text an agent reads.
    pub text: String,
    /// The tool's record, when the server sent one.
    pub record: Option<Value>,
}

/// A remote MCP connection to one instance with one token.
#[derive(Debug, Clone)]
pub struct RemoteClient {
    endpoint: Url,
    token: Secret,
    client: Client,
}

impl RemoteClient {
    /// A client of `server` (its base URL) with `token`.
    ///
    /// # Errors
    /// An unusable URL (plain `http` only for loopback hosts) or HTTP client.
    pub fn new(server: &Url, token: Secret) -> Result<Self, RemoteError> {
        validate_server_url(server)?;
        install_crypto_provider();
        let mut endpoint = server.clone();
        endpoint.set_query(None);
        endpoint.set_fragment(None);
        endpoint
            .path_segments_mut()
            .map_err(|()| ApiError::InvalidServerUrl(server.to_string()))?
            .pop_if_empty()
            .push("mcp");
        let client = Client::builder()
            .user_agent(concat!("yulius-cli/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| RemoteError::Transport(e.to_string()))?;
        Ok(Self {
            endpoint,
            token,
            client,
        })
    }

    /// The `/mcp` endpoint this client posts to.
    #[must_use]
    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }

    /// One JSON-RPC request; its `result`. Never retried: a tool may have changed something.
    fn rpc(&self, method: &str, params: Value) -> Result<Value, RemoteError> {
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let response = self
            .client
            .post(self.endpoint.clone())
            .bearer_auth(self.token.expose())
            .header("accept", "application/json, text/event-stream")
            .json(&body)
            .send()
            .map_err(|e| RemoteError::Transport(format!("{}: {e}", self.endpoint)))?;
        let status = response.status();
        let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok());
        if let Some(to) = moved_base(
            &self.endpoint,
            status,
            header(MOVED_HEADER),
            header("location"),
        ) {
            return Err(RemoteError::Moved(to));
        }
        let mut bytes = Vec::new();
        response
            .take(MAX_ANSWER_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| RemoteError::Transport(e.to_string()))?;
        if bytes.len() as u64 > MAX_ANSWER_BYTES {
            return Err(RemoteError::Protocol(
                "the answer is larger than 2 MiB".into(),
            ));
        }
        let answer: Option<Value> = serde_json::from_slice(&bytes).ok();
        let message = answer
            .as_ref()
            .and_then(|a| a["error"]["message"].as_str())
            .map(str::to_owned);
        match status {
            StatusCode::OK => {}
            StatusCode::UNAUTHORIZED => return Err(RemoteError::Unauthenticated),
            StatusCode::FORBIDDEN => {
                return Err(RemoteError::Forbidden(
                    message.unwrap_or_else(|| "forbidden".into()),
                ));
            }
            StatusCode::TOO_MANY_REQUESTS => {
                return Err(RemoteError::RateLimited(
                    message.unwrap_or_else(|| "too many requests; wait a minute".into()),
                ));
            }
            other => {
                return Err(RemoteError::Transport(format!(
                    "HTTP {other} from {}{}",
                    self.endpoint,
                    message.map(|m| format!(": {m}")).unwrap_or_default()
                )));
            }
        }
        let answer = answer.ok_or_else(|| RemoteError::Protocol("not JSON".into()))?;
        if let Some(message) = message {
            return Err(RemoteError::Protocol(message));
        }
        answer
            .get("result")
            .cloned()
            .ok_or_else(|| RemoteError::Protocol("no result".into()))
    }

    /// `initialize`: the server's name, version and instructions.
    ///
    /// # Errors
    /// As every request.
    pub fn initialize(&self) -> Result<Value, RemoteError> {
        self.rpc(
            "initialize",
            json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "clientInfo": {"name": "yulius-cli", "version": env!("CARGO_PKG_VERSION")}}),
        )
    }

    /// `tools/list`: the tools this token is served (name, description, input schema).
    ///
    /// # Errors
    /// As every request.
    pub fn tools(&self) -> Result<Vec<Value>, RemoteError> {
        let result = self.rpc("tools/list", json!({}))?;
        result["tools"]
            .as_array()
            .cloned()
            .ok_or_else(|| RemoteError::Protocol("no tools".into()))
    }

    /// `tools/call` of `name` with `arguments`, asking for the tool's record too.
    ///
    /// # Errors
    /// As every request; a tool's own failure is [`ToolOutcome::is_error`].
    pub fn call(&self, name: &str, arguments: Value) -> Result<ToolOutcome, RemoteError> {
        let result = self.rpc(
            "tools/call",
            json!({"name": name, "arguments": arguments, "_meta": {"yulius/structured": true}}),
        )?;
        let text = result["content"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        Ok(ToolOutcome {
            is_error: result["isError"] == true,
            text,
            record: result.get("structuredContent").cloned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;
    use std::net::TcpListener;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn endpoint(url: &str) -> Option<Url> {
        Url::parse(url).ok()
    }

    fn follows(endpoint_url: &str, moved: Option<&str>, location: Option<&str>) -> Option<String> {
        let endpoint = endpoint(endpoint_url)?;
        moved_base(&endpoint, StatusCode::TEMPORARY_REDIRECT, moved, location).map(String::from)
    }

    /// SPEC-466 MVC-030: the old host's redirect (SPEC-460 MIG-041) names the new base URL.
    #[test]
    fn a_move_s_redirect_names_the_new_base_url() {
        let old = "https://old.example/mcp";
        assert_eq!(
            follows(
                old,
                Some("https://new.example"),
                Some("https://new.example/mcp")
            )
            .as_deref(),
            Some("https://new.example/")
        );
        // As Caddy writes the header and with a port; under a path the path stays.
        assert_eq!(
            follows(
                old,
                Some("https://new.example:8443/"),
                Some("https://new.example:8443/mcp")
            )
            .as_deref(),
            Some("https://new.example:8443/")
        );
        assert_eq!(
            follows(
                "https://old.example/yulius/mcp",
                Some("https://new.example"),
                Some("https://new.example/yulius/mcp")
            )
            .as_deref(),
            Some("https://new.example/yulius")
        );
        // Any redirect status, as long as it is one.
        for status in [301, 302, 303, 307, 308] {
            let status = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
            let endpoint = endpoint(old);
            assert!(
                endpoint
                    .and_then(|e| moved_base(
                        &e,
                        status,
                        Some("https://new.example"),
                        Some("https://new.example/mcp")
                    ))
                    .is_some(),
                "{status}"
            );
        }
        // A development server on loopback may move to another one.
        assert_eq!(
            follows(
                "http://127.0.0.1:8080/mcp",
                Some("http://127.0.0.1:9090"),
                Some("http://127.0.0.1:9090/mcp")
            )
            .as_deref(),
            Some("http://127.0.0.1:9090/")
        );
    }

    /// SPEC-466 MVC-031: nothing else is followed, so the token goes nowhere else.
    #[test]
    fn no_other_redirect_is_a_move() {
        let old = "https://old.example/mcp";
        let new = Some("https://new.example");
        let there = Some("https://new.example/mcp");
        for (moved, location, why) in [
            (None, there, "no header"),
            (new, None, "no Location"),
            (new, Some("https://new.example/elsewhere"), "another path"),
            (new, Some("https://new.example/mcp?token=1"), "a query"),
            (
                new,
                Some("https://evil.example/mcp"),
                "another host than the header",
            ),
            (new, Some("/mcp"), "a relative Location"),
            (
                Some("http://new.example"),
                Some("http://new.example/mcp"),
                "plain http",
            ),
            (
                Some("http://127.0.0.1:9"),
                Some("http://127.0.0.1:9/mcp"),
                "https to loopback http",
            ),
            (
                Some("https://new.example/x"),
                Some("https://new.example/x/mcp"),
                "a path in the header",
            ),
            (
                Some("https://new.example?x=1"),
                there,
                "a query in the header",
            ),
            (Some("https://user:pw@new.example"), there, "credentials"),
            (
                Some("ftp://new.example"),
                Some("ftp://new.example/mcp"),
                "another scheme",
            ),
            (Some("new.example"), there, "not a URL"),
            (
                Some("https://old.example"),
                Some("https://old.example/mcp"),
                "a redirect to itself",
            ),
            (Some(""), there, "an empty header"),
        ] {
            assert_eq!(follows(old, moved, location), None, "{why}");
        }
        // Not a redirect: the header alone moves nothing.
        let endpoint = endpoint(old);
        assert!(
            endpoint
                .and_then(|e| moved_base(&e, StatusCode::OK, new, there))
                .is_none()
        );
    }

    /// One answer for one request on a loopback port; what was asked.
    fn answer_once(
        response: String,
    ) -> std::io::Result<(Url, std::thread::JoinHandle<std::io::Result<String>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = Url::parse(&format!("http://{}", listener.local_addr()?))
            .map_err(std::io::Error::other)?;
        let served = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept()?;
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") && stream.read(&mut byte)? == 1 {
                request.push(byte[0]);
            }
            stream.write_all(response.as_bytes())?;
            Ok(String::from_utf8_lossy(&request).into_owned())
        });
        Ok((url, served))
    }

    /// The client reads a move from a real answer and follows nothing itself: the caller decides.
    #[test]
    fn the_client_reports_a_move_and_sends_the_token_nowhere() -> TestResult {
        let new = TcpListener::bind("127.0.0.1:0")?;
        new.set_nonblocking(true)?;
        let there = format!("http://{}", new.local_addr()?);
        let (old, served) = answer_once(format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: {there}/mcp\r\nX-Yulius-Moved-To: \
             {there}\r\nContent-Length: 0\r\n\r\n"
        ))?;
        let client = RemoteClient::new(&old, Secret::new("yul_token"))?;
        match client.tools() {
            Err(RemoteError::Moved(to)) => assert_eq!(to.as_str(), format!("{there}/")),
            other => return Err(format!("expected a move, got {other:?}").into()),
        }
        let request = served.join().map_err(|_| "the old server died")??;
        assert!(request.starts_with("POST /mcp HTTP/1.1\r\n"), "{request}");
        // Nobody connected to the new server.
        assert!(new.accept().is_err());

        // A redirect that is not a move stays the error it was.
        let (old, served) = answer_once(format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: {there}/mcp\r\nContent-Length: 0\r\n\r\n"
        ))?;
        let client = RemoteClient::new(&old, Secret::new("yul_token"))?;
        match client.tools() {
            Err(RemoteError::Transport(why)) => assert!(why.contains("HTTP 307"), "{why}"),
            other => return Err(format!("expected HTTP 307, got {other:?}").into()),
        }
        served.join().map_err(|_| "the old server died")??;
        assert!(new.accept().is_err());
        Ok(())
    }
}
