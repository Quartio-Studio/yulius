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

/// A failed remote request.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    /// The server URL is unusable.
    #[error(transparent)]
    Url(#[from] ApiError),
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
