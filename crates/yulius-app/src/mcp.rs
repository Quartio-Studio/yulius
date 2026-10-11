//! Yulius MCP server: JSON-RPC dispatch for agent tools (SPEC-002 §7.4).

use std::sync::Arc;

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
            name: "sessions_search".into(),
            description: "Sessions are this project's docs: search the transcripts of earlier and parallel Yulius tasks and the Claude Code sessions people imported from their computers (full text: words from file names, functions, errors, features). Use it whenever you do not know how or why something was implemented, before guessing or re-deriving. Returns sessions with matching passages and their line numbers; read around a match with session_read.".into(),
            input_schema: schema(json!({
                "query": {"type": "string", "description": "Words to find (all must match; falls back to any)"},
                "kind": {"type": "string", "enum": ["all", "task", "imported"], "description": "Only task sessions or only imported ones (default all)"},
                "limit": {"type": "integer", "minimum": 1, "maximum": crate::session_docs::MAX_SEARCH_RESULTS, "description": "Most sessions (default 8)"}
            }), &["query"]),
        },
        ToolDescriptor {
            name: "session_read".into(),
            description: "Read part of one session of this project: by `id` from sessions_search, or the latest session of another task by `task_number`. Returns messages, tool calls, edits and results from line `from` (bounded); continue with `next_from`.".into(),
            input_schema: schema(json!({
                "id": {"type": "string", "description": "Session id from sessions_search"},
                "task_number": {"type": "integer", "description": "Read this task's latest session instead"},
                "from": {"type": "integer", "minimum": 0, "description": "First transcript line (default 0; use a match's line minus a few)"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 200, "description": "Most entries (default 60)"},
                "max_chars": {"type": "integer", "minimum": 1000, "maximum": crate::session_docs::READ_CHARS_MAX, "description": "Most characters of text (default 24000)"}
            }), &[]),
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
            description: "Validate this task's spec in the Yulius format (frontmatter, required sections, a Plan of at most 30 unique steps) and the project documentation; says whether this task needs a spec at all (features and fixes do; research, questions, docs and chores do not) and returns the problems to fix plus nudges about the plan. The same check runs when you stop.".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "steps_set".into(),
            description: "Replace this task's plan with an ordered list of 1-30 steps of similar effort (strings or {title, status, effort}), each a slice that leaves the project working, named by what it delivers. Keep it in sync with the Plan checklist of your spec. effort is 1 (default) to 3, only for a step that cannot be split; progress is weighted by it. Statuses: pending, in_progress, done, skipped (blocked needs step_update with a note). A step matching a current title keeps its status and effort; a finished step stays finished.".into(),
            input_schema: schema(json!({"steps": {"type": "array", "maxItems": crate::coordination::MAX_STEPS, "items": {"anyOf": [{"type": "string"}, {"type": "object", "properties": {"title": {"type": "string"}, "status": {"type": "string", "enum": ["pending", "in_progress", "done", "skipped"]}, "effort": {"type": "integer", "minimum": 1, "maximum": 3}}, "required": ["title"]}]}}}), &["steps"]),
        },
        ToolDescriptor {
            name: "step_update".into(),
            description: "Update one step (by step_id, ordinal or title). Mark a step in_progress when you start it and done the moment it is finished, one at a time (at most 2 in progress), so progress arrives steadily. A finished step stays finished: add a follow-up step with steps_set instead. blocked and skipped need a note saying why. Returns the steps, the progress and nudges.".into(),
            input_schema: schema(json!({"step_id": {"type": "string"}, "ordinal": {"type": "integer"}, "title": {"type": "string"}, "status": {"type": "string", "enum": ["pending", "in_progress", "done", "skipped", "blocked"]}, "note": {"type": "string"}}), &["status"]),
        },
        ToolDescriptor {
            name: "steps_get".into(),
            description: "Get this task's steps, the effort-weighted progress (percent) and nudges about the plan and its pace.".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "delivery_status".into(),
            description: "Delivery state of this task: pull request, checks, spec validity, open steps, the project's delivery profile and the next actions. Deliver with delivery_merge, delivery_deploy and delivery_promote.".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "ci_status".into(),
            description: "This project's CI, run by Yulius (the only CI; no GitHub Actions): the jobs in force and where they come from, the latest runs (this task's pull request first) and their state. With run_id, one run with the end of each job's output. A merge that requires checks waits until the latest run at the pull request head passed.".into(),
            input_schema: schema(json!({"run_id": {"type": "string", "description": "A run id from the list, to read its jobs' output"}}), &[]),
        },
        ToolDescriptor {
            name: "ci_rerun".into(),
            description: "Run the CI again for the commit of a finished run (for example after a failure that was not caused by the code). Refused while a run of that commit is queued or running.".into(),
            input_schema: schema(json!({"run_id": {"type": "string"}}), &["run_id"]),
        },
        ToolDescriptor {
            name: "ci_setup".into(),
            description: "Onboarding: move the project's GitHub Actions into Yulius CI. Returns the workflows in this task's worktree (jobs, run steps, uses steps, matrices), the CI its yulius.toml configures ([[ci.jobs]] or [commands] gate) and the steps: translate every check into [[ci.jobs]], delete .github/workflows/, commit; the pull request is tested with the CI it adds.".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "repo_hygiene".into(),
            description: "Stale branches and worktrees of this project: remote branches whose pull request was merged or closed, branches already merged, local branches no worktree uses, worktrees of deleted, archived or merged tasks. Each item has a reason and whether removing it is safe.".into(),
            input_schema: schema(json!({"refresh": {"type": "boolean", "description": "Scan now instead of returning the last scan"}}), &[]),
        },
        ToolDescriptor {
            name: "repo_cleanup".into(),
            description: "Remove stale branches and worktrees reported by repo_hygiene: the given items, or every safe item when items is omitted. Unmerged branches without a pull request (reason idle) are removed only with force. Live tasks' worktrees, protected branches and branches with an open pull request are never removed; dirty worktrees are kept.".into(),
            input_schema: schema(json!({
                "items": {"type": "array", "items": {"type": "object", "properties": {"kind": {"type": "string", "enum": ["remote_branch", "local_branch", "worktree"]}, "name": {"type": "string"}}, "required": ["kind", "name"]}},
                "force": {"type": "boolean"}
            }), &[]),
        },
        ToolDescriptor {
            name: "delivery_merge".into(),
            description: "Merge this task's pull request on green (the one delivery pipeline: you deliver your own work, no person accepts it). Refused unless the checks passed on the exact head (Yulius CI), the spec is valid when the task needs one, every step is done, the head is current and an independent review in a fresh session approved exactly this head: pass that commit as reviewed_head (and a one-line review_summary). Acts with the authority of the task's creator and is audited.".into(),
            input_schema: schema(json!({
                "reviewed_head": {"type": "string", "description": "The commit SHA the independent reviewer approved (must be the branch head)"},
                "review_summary": {"type": "string", "description": "One line: the reviewer's verdict and what it checked"}
            }), &["reviewed_head"]),
        },
        ToolDescriptor {
            name: "delivery_deploy".into(),
            description: "Run the next environment of the delivery profile (for example dev after a merge, production after a promotion): its deploy command runs and its verification decides. Only environments with a command and verify = \"command\"; one a person must verify is refused. Follow the run with delivery_status.".into(),
            input_schema: schema(json!({"environment": {"type": "string", "description": "Environment name from the delivery profile"}}), &["environment"]),
        },
        ToolDescriptor {
            name: "delivery_promote".into(),
            description: "Promote the integration branch to production once every merged task's integration environments are done. With merge-commit promotion the first call opens the promotion pull request and a later call, after its checks passed, merges it; fast-forward promotes at once.".into(),
            input_schema: schema(json!({}), &[]),
        },
        ToolDescriptor {
            name: "issue_create".into(),
            description: "Open a GitHub issue for follow-up work, a bug or a flaky test you found, instead of widening this task. It must meet the Definition of Ready: exactly one `type: …` label (bug, enhancement, docs, chore, performance), one `priority: …` label (high, medium, low), at least one `area: …` label (add `risk: high` for authentication, permissions, data isolation, migrations, money or shared contracts), and a body with a summary, current and expected behavior and a `## Acceptance criteria` checklist. Missing labels are created.".into(),
            input_schema: schema(json!({
                "title": {"type": "string"},
                "body": {"type": "string", "description": "Markdown with a ## Acceptance criteria checklist"},
                "labels": {"type": "array", "items": {"type": "string"}, "maxItems": 10}
            }), &["title", "body", "labels"]),
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

/// Agent-secret tools (SPEC-002 ADR-20): names only, and typing into the sandbox browser.
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
    pub async fn mcp_handle(
        self: &Arc<Self>,
        token: &str,
        request: RpcRequest,
    ) -> Option<RpcResponse> {
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
                    "instructions": "Yulius coordinates this project's tasks. Use tasks_list and sessions_search before starting, message_send to coordinate with other tasks, and steps_set/step_update to keep a plan of similar-effort steps whose progress arrives steadily.",
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

    async fn mcp_call(self: &Arc<Self>, ctx: &AgentContext, params: &Value) -> Value {
        let name = params.get("name").and_then(Value::as_str).unwrap_or("");
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let outcome: AppResult<Value> = match name {
            "project_context" => self.tool_project_context(ctx).await,
            "tasks_list" => self.tool_tasks_list(ctx, &args).await,
            "task_get" => self.tool_task_get(ctx, &args).await,
            "sessions_search" => self.tool_sessions_search(ctx, &args).await,
            "session_read" => self.tool_session_read(ctx, &args).await,
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
            "ci_status" => self.tool_ci_status(ctx, &args).await,
            "ci_rerun" => self.tool_ci_rerun(ctx, &args).await,
            "ci_setup" => self.tool_ci_setup(ctx).await,
            "repo_hygiene" => self.tool_repo_hygiene(ctx, &args).await,
            "repo_cleanup" => self.tool_repo_cleanup(ctx, &args).await,
            "delivery_merge" => self.tool_delivery_merge(ctx, &args).await,
            "delivery_deploy" => self.tool_delivery_deploy(ctx, &args).await,
            "delivery_promote" => self.tool_delivery_promote(ctx).await,
            "issue_create" => self.tool_issue_create(ctx, &args).await,
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
                // No tool result carries an agent secret's value (SPEC-002 ADR-20): page text,
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
