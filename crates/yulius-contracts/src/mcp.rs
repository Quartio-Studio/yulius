//! Yulius MCP JSON-RPC messages and tool descriptors (SPEC-002 §7.4).
//!
//! Streamable HTTP transport, JSON responses only (no server-initiated streams).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol versions the Yulius MCP server accepts (newest first).
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 2] = ["2025-06-18", "2025-03-26"];

/// JSON-RPC request or notification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcRequest {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Request id; absent for notifications.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    /// Method name.
    pub method: String,
    /// Parameters.
    #[serde(default)]
    pub params: Value,
}

/// JSON-RPC response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcResponse {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Echoed id.
    pub id: Value,
    /// Result on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    /// Error code (`-32601` method not found, `-32602` invalid params, `-32603` internal).
    pub code: i64,
    /// Message.
    pub message: String,
}

impl RpcResponse {
    /// Successful response.
    #[must_use]
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Error response.
    #[must_use]
    pub fn err(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

/// A tool descriptor for `tools/list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDescriptor {
    /// Tool name.
    pub name: String,
    /// What the tool does (shown to the model).
    pub description: String,
    /// JSON Schema of the arguments.
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// Builds a `tools/call` result with one text content item.
#[must_use]
pub fn text_result(text: impl Into<String>, is_error: bool) -> Value {
    serde_json::json!({
        "content": [{"type": "text", "text": text.into()}],
        "isError": is_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_response_shapes() -> Result<(), serde_json::Error> {
        let req: RpcRequest = serde_json::from_str(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"tasks_list"}}"#,
        )?;
        assert_eq!(req.method, "tools/call");
        let note: RpcRequest =
            serde_json::from_str(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)?;
        assert!(note.id.is_none());
        let resp = serde_json::to_value(RpcResponse::err(Value::from(1), -32601, "nope"))?;
        assert_eq!(resp["error"]["code"], -32601);
        assert!(resp.get("result").is_none());
        Ok(())
    }
}
