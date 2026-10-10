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
        obj.insert("to".into(), json!({"type": "string", "enum": ["all", "people"], "description": "\"all\" broadcasts to the other tasks of the sandbox; \"people\" tells or asks the people using Yulius something: it shows in this task's Messages and in their notifications, and they reply in this session's terminal"}));
        obj.insert(
            "kind".into(),
            json!({"type": "string", "enum": ["info", "question", "request", "scope"]}),
        );
        obj.insert("subject".into(), json!({"type": "string"}));
        obj.insert("body".into(), json!({"type": "string"}));
    }
    let mut tools = vec![
        ToolDescriptor {
            name: "project_context".into(),
            description: "The project context, rebuilt now: project summary, architecture and decision index, recent changes, delivery rules and the parallel tasks with their scope claims (the same briefing the session started with).".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "tasks_list".into(),
            description: "List previous and parallel tasks of this project (other Claude Code sessions): status, branch, spec and step progress. Use it before starting work to avoid duplicating or conflicting with others.".into(),
            input_schema: schema(json!({"scope": {"type": "string", "enum": ["all", "active", "recent", "archived"]}, "limit": {"type": "integer"}}), &[]),
        },
        ToolDescriptor {
            name: "task_get".into(),
            description: "Get one task's details: prompt, spec path, branch and its step list.".into(),
            input_schema: schema(task_ref.clone(), &[]),
        },
        ToolDescriptor {
            name: "message_send".into(),
            description: "Send a message to another task (by task_id or number), broadcast to the other tasks with to=\"all\", or tell or ask the people using Yulius something with to=\"people\" (they reply in this session's terminal, not by message). Use kind=scope to agree who works on which files.".into(),
            input_schema: schema(message_props, &["body"]),
        },
        ToolDescriptor {
            name: "messages_inbox".into(),
            description: "Read messages sent to this task by other tasks; marks them read.".into(),
            input_schema: schema(json!({"unread_only": {"type": "boolean"}}), &[]),
        },
        ToolDescriptor {
            name: "scope_claim".into(),
            description: "Claim repository paths this task is about to change, so parallel tasks avoid conflicts: path globs relative to the repository (* within a segment, ** across segments, a trailing / claims a directory, e.g. \"src/api/\", \"docs/**/*.md\"). Claims are advisory and never block files. Overlaps with other live tasks' claims are returned and reported to those tasks; agree who changes what with message_send kind=scope.".into(),
            input_schema: schema(json!({
                "pattern": {"type": "string", "description": "One path glob"},
                "patterns": {"type": "array", "items": {"type": "string"}, "maxItems": crate::scopes::MAX_PATTERNS_PER_CLAIM, "description": "Several path globs"},
                "note": {"type": "string", "description": "Why this task needs these paths"}
            }), &[]),
        },
        ToolDescriptor {
            name: "scope_release".into(),
            description: "Release this task's scope claims named by pattern (one glob) or patterns (a list), or all of them when neither is given, e.g. when the work on those paths is done.".into(),
            input_schema: schema(json!({
                "pattern": {"type": "string"},
                "patterns": {"type": "array", "items": {"type": "string"}, "maxItems": crate::scopes::MAX_PATTERNS_PER_CLAIM}
            }), &[]),
        },
        ToolDescriptor {
            name: "scopes_list".into(),
            description: "Current scope claims of the live tasks in this project (yours marked mine) and the overlaps with your claims.".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "spec_check".into(),
            description: "Validate this task's spec in the Yulius format (frontmatter, required sections, Plan checklist) and the project documentation; returns the problems to fix. The same check runs when you stop.".into(),
            input_schema: schema(json!({}), &[]),
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
        ToolDescriptor {
            name: "delivery_status".into(),
            description: "Read-only delivery state of this task: pull request, checks, spec validity, open steps, the project's delivery profile and what people can do next. Merging, deploying and promoting are human actions in Yulius; there is no tool for them.".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "automation_create".into(),
            description: "Create a scheduled automation in this project: on a five-field cron schedule (minute hour day-of-month month day-of-week, evaluated at utc_offset_minutes from UTC) Yulius creates and starts a new task with the given prompt. Use it when the user asks for recurring work, e.g. \"every day at 9 implement yesterday's customer feedback\". The automation appears on the project's Automations page.".into(),
            input_schema: schema(json!({
                "name": {"type": "string", "description": "Short name; also the title of created tasks"},
                "cron": {"type": "string", "description": "Five-field cron expression, e.g. \"0 9 * * *\" (daily at 09:00) or \"0 9 * * MON-FRI\""},
                "utc_offset_minutes": {"type": "integer", "description": "UTC offset of the user's time zone in minutes (e.g. 120 for UTC+2); default 0"},
                "prompt": {"type": "string", "description": "What each created task should do"},
                "harness": {"type": "string", "enum": ["claude"], "description": "Agent for created tasks (Claude Code; the default)"},
                "workflow": {"type": "string", "description": "Workflow id or name from workflows_list (optional)"},
                "enabled": {"type": "boolean", "description": "Default true"}
            }), &["name", "cron", "prompt"]),
        },
        ToolDescriptor {
            name: "automations_list".into(),
            description: "List this project's automations with schedule, next run and last run.".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "automation_update".into(),
            description: "Change an automation of this project: enable/disable it, or change its cron schedule, UTC offset, prompt or name.".into(),
            input_schema: schema(json!({
                "automation_id": {"type": "string"},
                "enabled": {"type": "boolean"},
                "cron": {"type": "string"},
                "utc_offset_minutes": {"type": "integer"},
                "prompt": {"type": "string"},
                "name": {"type": "string"}
            }), &["automation_id"]),
        },
        ToolDescriptor {
            name: "workflows_list".into(),
            description: "List the workflows (reusable task recipes with initial steps, instructions and gates) available in this project: Yulius's defaults first (builtin, with when_to_use), then the project's own.".into(),
            input_schema: schema(json!({}), &[]),
        },
    ];
    tools.extend(crate::browser::tool_descriptors());
    tools.extend(secret_tool_descriptors());
    tools
}

/// Agent-secret tools (SPEC-002 ADR-18): names only, and typing into the sandbox browser.
fn secret_tool_descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: "secrets_list".into(),
            description: "List the agent secrets this project may use: names, descriptions, the hosts each is bound to and how it is added to requests. You never receive values: Yulius' proxy adds a secret to HTTP requests for its bound hosts (call them normally; in code use the environment variable NAME, which holds a placeholder), browser_fill_secret types it into a sign-in page, and any value that would appear in output shows as [secret:NAME].".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "browser_fill_secret".into(),
            description: "Type an agent secret into the project browser without seeing it: its value (password or key), its user name, or its current two-factor (TOTP) code, into the focused field or the element matching selector, in a tab whose page is on one of the secret's bound hosts. Use browser_snapshot first to find the field.".into(),
            input_schema: schema(json!({
                "tab_id": {"type": "string", "description": "Tab from browser_tabs or browser_open"},
                "secret": {"type": "string", "description": "Secret NAME from secrets_list"},
                "field": {"type": "string", "enum": ["value", "password", "username", "totp"], "description": "What to type (default value; password is the same as value)"},
                "selector": {"type": "string", "description": "CSS selector of the field to focus first"}
            }), &["tab_id", "secret"]),
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
            "project_context" => self.tool_project_context(ctx).await,
            "tasks_list" => self.tool_tasks_list(ctx, &args).await,
            "task_get" => self.tool_task_get(ctx, &args).await,
            "message_send" => self.tool_message_send(ctx, &args).await,
            "messages_inbox" => self.tool_messages_inbox(ctx, &args).await,
            "scope_claim" => self.tool_scope_claim(ctx, &args).await,
            "scope_release" => self.tool_scope_release(ctx, &args).await,
            "scopes_list" => self.tool_scopes_list(ctx).await,
            "spec_check" => self.tool_spec_check(ctx).await,
            "steps_set" => self.tool_steps_set(ctx, &args).await,
            "step_update" => self.tool_step_update(ctx, &args).await,
            "steps_get" => self.tool_steps_get(ctx).await,
            "delivery_status" => self.tool_delivery_status(ctx).await,
            "automation_create" => self.tool_automation_create(ctx, &args).await,
            "automations_list" => self.tool_automations_list(ctx).await,
            "automation_update" => self.tool_automation_update(ctx, &args).await,
            "workflows_list" => self.tool_workflows_list(ctx).await,
            "browser_tabs" => self.tool_browser_tabs(ctx, &args).await,
            "browser_open" => self.tool_browser_open(ctx, &args).await,
            "browser_navigate" => self.tool_browser_navigate(ctx, &args).await,
            "browser_click" => self.tool_browser_click(ctx, &args).await,
            "browser_type" => self.tool_browser_type(ctx, &args).await,
            "browser_snapshot" => self.tool_browser_snapshot(ctx, &args).await,
            // Returns a complete result with an image item (size-bounded by the tool itself).
            "browser_screenshot" => return self.tool_browser_screenshot(ctx, &args).await,
            "browser_request_user_action" => {
                self.tool_browser_request_user_action(ctx, &args).await
            }
            "secrets_list" => self.tool_secrets_list(ctx).await,
            "browser_fill_secret" => self.tool_browser_fill_secret(ctx, &args).await,
            other => Err(AppError::Invalid(format!("unknown tool: {other}"))),
        };
        match outcome {
            Ok(value) => {
                // No tool result carries an agent secret's value (SPEC-002 ADR-18): page text,
                // messages, task prompts and everything else are redacted before the agent.
                let text = serde_json::to_string_pretty(&value).unwrap_or_default();
                let mut text = self.redact_for_sandbox(&ctx.task.sandbox_id, &text).await;
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
            Err(err) => text_result(
                self.redact_for_sandbox(&ctx.task.sandbox_id, &err.public_message())
                    .await,
                true,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tool_descriptors;
    use yulius_domain::briefing::YULIUS_TOOLS;

    #[test]
    fn every_tool_the_briefing_announces_is_served() {
        let served: Vec<String> = tool_descriptors().into_iter().map(|t| t.name).collect();
        for (name, _) in YULIUS_TOOLS {
            assert!(
                served.iter().any(|s| s == name),
                "the briefing announces `{name}`, which tools/list does not serve"
            );
        }
    }
}
