//! Yulius MCP server: JSON-RPC dispatch for agent tools (SPEC-002 §7.4).

use serde_json::{Value, json};
use yulius_contracts::mcp::{
    RpcRequest, RpcResponse, SUPPORTED_PROTOCOL_VERSIONS, ToolDescriptor, text_result,
};

use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};

/// Maximum tool result text returned to the model.
pub const MAX_RESULT_BYTES: usize = 256 * 1024;

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false})
}

/// Tool descriptors served by `tools/list`.
#[must_use]
pub fn tool_descriptors() -> Vec<ToolDescriptor> {
    let task_ref = json!({
        "task_id": {"type": "string", "description": "Task id"},
        "number": {"type": "integer", "description": "Task number within this sandbox"}
    });
    let mut message_props = task_ref.clone();
    if let Some(obj) = message_props.as_object_mut() {
        obj.insert("to".into(), json!({"type": "string", "enum": ["all"], "description": "Use \"all\" to broadcast to every active task"}));
        obj.insert(
            "kind".into(),
            json!({"type": "string", "enum": ["info", "question", "request", "scope"]}),
        );
        obj.insert("subject".into(), json!({"type": "string"}));
        obj.insert("body".into(), json!({"type": "string"}));
    }
    vec![
        ToolDescriptor {
            name: "tasks_list".into(),
            description: "List previous and parallel tasks of this project (other Claude Code or Codex sessions): status, branch, spec and step progress. Use it before starting work to avoid duplicating or conflicting with others.".into(),
            input_schema: schema(json!({"scope": {"type": "string", "enum": ["all", "active", "recent", "archived"]}, "limit": {"type": "integer"}}), &[]),
        },
        ToolDescriptor {
            name: "task_get".into(),
            description: "Get one task's details: prompt, spec path, branch and its step list.".into(),
            input_schema: schema(task_ref.clone(), &[]),
        },
        ToolDescriptor {
            name: "message_send".into(),
            description: "Send a message to another task (by task_id or number) or broadcast with to=\"all\". Works across Claude Code and Codex. Use kind=scope to agree who works on which files.".into(),
            input_schema: schema(message_props, &["body"]),
        },
        ToolDescriptor {
            name: "messages_inbox".into(),
            description: "Read messages sent to this task by other tasks or by people; marks them read.".into(),
            input_schema: schema(json!({"unread_only": {"type": "boolean"}}), &[]),
        },
        ToolDescriptor {
            name: "steps_set".into(),
            description: "Replace this task's plan with an ordered list of steps (strings or {title,status}). Keep it in sync with the Plan checklist of your spec. Statuses: pending, in_progress, done, skipped, blocked.".into(),
            input_schema: schema(json!({"steps": {"type": "array", "items": {"anyOf": [{"type": "string"}, {"type": "object", "properties": {"title": {"type": "string"}, "status": {"type": "string"}}, "required": ["title"]}]}}}), &["steps"]),
        },
        ToolDescriptor {
            name: "step_update".into(),
            description: "Update one step (by step_id, ordinal or exact title) to a new status with an optional note. Mark a step in_progress when you start it and done when finished.".into(),
            input_schema: schema(json!({"step_id": {"type": "string"}, "ordinal": {"type": "integer"}, "title": {"type": "string"}, "status": {"type": "string", "enum": ["pending", "in_progress", "done", "skipped", "blocked"]}, "note": {"type": "string"}}), &["status"]),
        },
        ToolDescriptor {
            name: "steps_get".into(),
            description: "Get this task's steps and progress.".into(),
            input_schema: schema(json!({}), &[]),
        },
    ]
}

impl App {
    /// Handles one JSON-RPC message from an agent; `None` for notifications.
    pub async fn mcp_handle(&self, token: &str, request: RpcRequest) -> Option<RpcResponse> {
        let id = request.id.clone()?;
        let ctx = match self.agent_context(token).await {
            Ok(ctx) => ctx,
            Err(_) => {
                return Some(RpcResponse::err(
                    id,
                    -32001,
                    "invalid or expired task token",
                ));
            }
        };
        let result: AppResult<Value> = match request.method.as_str() {
            "initialize" => {
                let requested = request
                    .params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let version = SUPPORTED_PROTOCOL_VERSIONS
                    .iter()
                    .find(|v| **v == requested)
                    .copied()
                    .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "yulius", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": "Yulius coordinates this project's tasks. Use tasks_list before starting, message_send to coordinate with other tasks, and steps_set/step_update to keep your plan visible.",
                }))
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tool_descriptors()})),
            "tools/call" => Ok(self.mcp_call(&ctx, &request.params).await),
            other => {
                return Some(RpcResponse::err(
                    id,
                    -32601,
                    format!("method not found: {other}"),
                ));
            }
        };
        Some(match result {
            Ok(value) => RpcResponse::ok(id, value),
            Err(err) => RpcResponse::err(id, -32603, err.public_message()),
        })
    }

    async fn mcp_call(&self, ctx: &AgentContext, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let outcome: AppResult<Value> = match name {
            "tasks_list" => self.tool_tasks_list(ctx, &args).await,
            "task_get" => self.tool_task_get(ctx, &args).await,
            "message_send" => self.tool_message_send(ctx, &args).await,
            "messages_inbox" => self.tool_messages_inbox(ctx, &args).await,
            "steps_set" => self.tool_steps_set(ctx, &args).await,
            "step_update" => self.tool_step_update(ctx, &args).await,
            "steps_get" => self.tool_steps_get(ctx).await,
            other => Err(AppError::Invalid(format!("unknown tool: {other}"))),
        };
        match outcome {
            Ok(value) => {
                let mut text = serde_json::to_string_pretty(&value).unwrap_or_default();
                if text.len() > MAX_RESULT_BYTES {
                    let mut cut = MAX_RESULT_BYTES;
                    while !text.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    text.truncate(cut);
                    text.push_str("\n…(truncated)");
                }
                text_result(text, false)
            }
            Err(err) => text_result(err.public_message(), true),
        }
    }
}
