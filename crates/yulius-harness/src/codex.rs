//! Codex rollout JSONL (structure of Codex CLI 0.160.1): observation, reading into the IR and
//! writing a resumable rollout (SPEC-002 §7.2, §7.3).
//!
//! Each line is `{timestamp, type, payload}` (newer builds add `ordinal`). Types used here:
//! `session_meta`, `turn_context`, `response_item` and `event_msg`; others are ignored.

use std::path::PathBuf;

use serde_json::{Map, Value, json};
use yulius_domain::harness::Harness;
use yulius_domain::time::Timestamp;
use yulius_domain::transcript::{Block, Role, Transcript, TranscriptMeta};
use yulius_domain::usage::{AccountUsage, TokenUsage, UsageWindow};

use crate::ir::push_blocks;
use crate::json::{
    SUMMARY_READ_BYTES, compact, obj_of, owned_str, parse_object, shorten, str_of, timestamp_of,
    u64_of,
};
use crate::observe::Observed;
use crate::time::{civil, format_rfc3339, from_epoch_seconds};

/// Prefixes of user-role messages that Codex injects as context rather than typed prompts.
/// Heuristic, verified only against the structure description of 0.160.1.
const INJECTED_CONTEXT_PREFIXES: [&str; 5] = [
    "<environment_context",
    "<user_instructions",
    "# AGENTS.md instructions",
    "<turn_aborted",
    "<user_shell_command",
];

/// Normalizes one Codex rollout line.
#[must_use]
pub fn observe_line(line: &str) -> Vec<Observed> {
    let Some(record) = parse_object(line) else {
        return vec![Observed::Malformed];
    };
    let Some(kind) = str_of(&record, "type") else {
        return vec![Observed::Malformed];
    };
    let known = matches!(
        kind,
        "session_meta" | "turn_context" | "response_item" | "event_msg"
    );
    if !known {
        return Vec::new();
    }
    let Some(payload) = obj_of(&record, "payload") else {
        return vec![Observed::Malformed];
    };
    match kind {
        "session_meta" => match owned_str(payload, "id") {
            Some(session_id) => vec![Observed::SessionMeta {
                session_id,
                cwd: owned_str(payload, "cwd"),
                model: owned_str(payload, "model"),
                version: owned_str(payload, "cli_version"),
            }],
            None => vec![Observed::Malformed],
        },
        "turn_context" => vec![Observed::TurnContext {
            cwd: owned_str(payload, "cwd"),
            model: owned_str(payload, "model"),
            effort: owned_str(payload, "effort"),
        }],
        "response_item" => {
            let is_user_prompt = str_of(payload, "type") == Some("message")
                && str_of(payload, "role") == Some("user")
                && message_texts(payload)
                    .iter()
                    .any(|t| !is_injected_context(t));
            if is_user_prompt {
                vec![Observed::UserTurn]
            } else {
                Vec::new()
            }
        }
        _ => observe_event(payload, timestamp_of(&record, "timestamp")),
    }
}

fn observe_event(payload: &Map<String, Value>, at: Option<Timestamp>) -> Vec<Observed> {
    match str_of(payload, "type") {
        Some("token_count") => {
            let mut out = Vec::new();
            if let Some(total) =
                obj_of(payload, "info").and_then(|info| obj_of(info, "total_token_usage"))
            {
                out.push(Observed::CumulativeUsage(token_usage(total)));
            }
            if let Some(limits) = obj_of(payload, "rate_limits") {
                let usage = account_usage(limits, at);
                if usage.limit_reached {
                    let kind = str_of(limits, "rate_limit_reached_type").unwrap_or("unknown");
                    out.push(Observed::RateLimits(usage));
                    out.push(Observed::LimitReached {
                        resets_at: usage.limit_resets_at,
                        message: format!("Codex rate limit reached ({})", shorten(kind, 64)),
                    });
                } else {
                    out.push(Observed::RateLimits(usage));
                }
            }
            out
        }
        Some("task_complete") => vec![Observed::TurnComplete {
            duration_ms: u64_of(payload, "duration_ms"),
        }],
        _ => Vec::new(),
    }
}

/// Codex counts cached tokens inside `input_tokens`; the IR's `input` is uncached input.
fn token_usage(total: &Map<String, Value>) -> TokenUsage {
    let input = u64_of(total, "input_tokens");
    let cached = u64_of(total, "cached_input_tokens");
    TokenUsage {
        input: match (input, cached) {
            (Some(i), Some(c)) => Some(i.saturating_sub(c)),
            (Some(i), None) => Some(i),
            (None, _) => None,
        },
        output: u64_of(total, "output_tokens"),
        cache_read: cached,
        cache_write: None,
    }
}

fn window(value: Option<&Value>) -> Option<UsageWindow> {
    let window = value?.as_object()?;
    Some(UsageWindow {
        used_percent: window.get("used_percent").and_then(Value::as_f64),
        window_minutes: u64_of(window, "window_minutes").and_then(|m| u32::try_from(m).ok()),
        resets_at: window
            .get("resets_at")
            .and_then(Value::as_i64)
            .and_then(from_epoch_seconds),
    })
}

fn account_usage(limits: &Map<String, Value>, at: Option<Timestamp>) -> AccountUsage {
    let primary = window(limits.get("primary"));
    let secondary = window(limits.get("secondary"));
    let limit_reached = str_of(limits, "rate_limit_reached_type").is_some_and(|t| !t.is_empty());
    let limit_resets_at = if limit_reached {
        let windows: Vec<UsageWindow> = primary.iter().chain(secondary.iter()).copied().collect();
        let full = windows
            .iter()
            .filter(|w| w.known_percent().is_some_and(|p| p >= 100.0))
            .filter_map(|w| w.resets_at)
            .max();
        // Without a full window, wait for the latest known reset (conservative).
        full.or_else(|| windows.iter().filter_map(|w| w.resets_at).max())
    } else {
        None
    };
    AccountUsage {
        primary,
        secondary,
        limit_reached,
        limit_resets_at,
        observed_at: at,
    }
}

fn message_texts(payload: &Map<String, Value>) -> Vec<&str> {
    match payload.get("content") {
        Some(Value::Array(items)) => items
            .iter()
            .filter(|i| {
                matches!(
                    i.get("type").and_then(Value::as_str),
                    Some("input_text" | "output_text" | "text")
                )
            })
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect(),
        Some(Value::String(text)) => vec![text.as_str()],
        _ => Vec::new(),
    }
}

fn is_injected_context(text: &str) -> bool {
    let trimmed = text.trim_start();
    INJECTED_CONTEXT_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

/// Reads a Codex rollout into the IR.
///
/// `session_meta` supplies the session id and cwd, the first `turn_context` the model. User
/// messages (minus injected context) and assistant messages become text; `function_call` and
/// `custom_tool_call` become tool calls and their outputs tool results in the assistant turn;
/// `reasoning` becomes [`Block::Omitted`]. Event messages, unknown and malformed lines are
/// ignored.
#[must_use]
pub fn read_transcript<I, S>(lines: I) -> Transcript
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut meta = TranscriptMeta {
        source: Harness::Codex,
        session_id: String::new(),
        cwd: None,
        model: None,
    };
    let mut turns = Vec::new();
    for line in lines {
        let Some(record) = parse_object(line.as_ref()) else {
            continue;
        };
        let Some(payload) = obj_of(&record, "payload") else {
            continue;
        };
        let at = timestamp_of(&record, "timestamp");
        match str_of(&record, "type") {
            Some("session_meta") => {
                if meta.session_id.is_empty()
                    && let Some(id) = str_of(payload, "id")
                {
                    id.clone_into(&mut meta.session_id);
                    meta.cwd = owned_str(payload, "cwd");
                }
            }
            Some("turn_context") => {
                if meta.model.is_none() {
                    meta.model = owned_str(payload, "model");
                }
            }
            Some("response_item") => read_item(payload, at, &mut turns),
            _ => {}
        }
    }
    Transcript { meta, turns }
}

fn read_item(
    payload: &Map<String, Value>,
    at: Option<Timestamp>,
    turns: &mut Vec<yulius_domain::transcript::Turn>,
) {
    match str_of(payload, "type") {
        Some("message") => {
            let role = match str_of(payload, "role") {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                _ => return, // developer/system instructions are not conversation
            };
            let blocks: Vec<Block> = message_texts(payload)
                .into_iter()
                .filter(|t| role == Role::Assistant || !is_injected_context(t))
                .map(|t| Block::Text(t.to_owned()))
                .collect();
            push_blocks(turns, role, blocks, at);
        }
        Some("function_call") => {
            let block = Block::ToolCall {
                name: str_of(payload, "name").unwrap_or("tool").to_owned(),
                input_summary: str_of(payload, "arguments")
                    .map_or_else(String::new, summarize_arguments),
            };
            push_blocks(turns, Role::Assistant, vec![block], at);
        }
        Some("custom_tool_call") => {
            let block = Block::ToolCall {
                name: str_of(payload, "name").unwrap_or("tool").to_owned(),
                input_summary: payload
                    .get("input")
                    .map_or_else(String::new, |v| compact(v, SUMMARY_READ_BYTES)),
            };
            push_blocks(turns, Role::Assistant, vec![block], at);
        }
        Some("function_call_output" | "custom_tool_call_output") => {
            let (summary, is_error) = tool_output(payload.get("output"));
            let block = Block::ToolResult {
                summary: shorten(&summary, SUMMARY_READ_BYTES),
                is_error,
            };
            push_blocks(turns, Role::Assistant, vec![block], at);
        }
        Some("reasoning") => {
            let block = Block::Omitted {
                what: "reasoning".into(),
            };
            push_blocks(turns, Role::Assistant, vec![block], at);
        }
        _ => {}
    }
}

/// Short rendering of `function_call.arguments` (a JSON string): the command when present.
fn summarize_arguments(arguments: &str) -> String {
    let Ok(Value::Object(args)) = serde_json::from_str::<Value>(arguments) else {
        return shorten(arguments, SUMMARY_READ_BYTES);
    };
    match args.get("command") {
        Some(Value::Array(parts)) => {
            let parts: Vec<&str> = parts.iter().filter_map(Value::as_str).collect();
            shorten(&parts.join(" "), SUMMARY_READ_BYTES)
        }
        Some(Value::String(command)) => shorten(command, SUMMARY_READ_BYTES),
        _ => shorten(arguments, SUMMARY_READ_BYTES),
    }
}

/// Text and error flag of a tool output: a plain string, a JSON string
/// `{"output": …, "metadata": {"exit_code": n}}`, or an object with `content`/`success`.
fn tool_output(output: Option<&Value>) -> (String, bool) {
    match output {
        Some(Value::String(text)) => match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(parsed)) if parsed.contains_key("output") => {
                let exit_code = parsed
                    .get("metadata")
                    .and_then(|m| m.get("exit_code"))
                    .and_then(Value::as_i64);
                let body = parsed
                    .get("output")
                    .map_or_else(String::new, |v| compact(v, SUMMARY_READ_BYTES));
                (body, exit_code.is_some_and(|c| c != 0))
            }
            _ => (text.clone(), false),
        },
        Some(Value::Object(object)) => {
            let body = object
                .get("content")
                .or_else(|| object.get("output"))
                .map_or_else(String::new, |v| compact(v, SUMMARY_READ_BYTES));
            let failed = object.get("success").and_then(Value::as_bool) == Some(false);
            (body, failed)
        }
        Some(other) if !other.is_null() => (compact(other, SUMMARY_READ_BYTES), false),
        _ => (String::new(), false),
    }
}

/// Parameters of [`write_rollout`].
#[derive(Debug, Clone, Copy)]
pub struct RolloutParams<'a> {
    /// Session id of the synthesized rollout (`session_meta.id`, file name suffix).
    pub session_id: &'a str,
    /// Working directory (the task worktree).
    pub cwd: &'a str,
    /// Codex CLI version recorded in `session_meta.cli_version`.
    pub cli_version: &'a str,
    /// Model recorded in `turn_context` (the model the resumed session will use).
    pub model: &'a str,
    /// Reasoning effort recorded in `turn_context`, if any.
    pub effort: Option<&'a str>,
    /// Timestamp of the first line in milliseconds since the epoch; each later line is one
    /// millisecond later.
    pub start_ms: i64,
}

/// Writes a Codex rollout that `codex resume <session_id>` can load.
///
/// Lines: `session_meta` (originator `yulius`, model provider `openai`), one `turn_context`,
/// then per exchange `event_msg task_started`, the user `response_item` message
/// (`input_text`), the assistant message (`output_text`) and `event_msg task_complete`.
/// Non-text IR blocks are rendered as text; empty turns are skipped. JSONL with a trailing
/// newline.
#[must_use]
pub fn write_rollout(transcript: &Transcript, params: &RolloutParams<'_>) -> String {
    let mut lines: Vec<Value> = Vec::new();
    let mut clock = params.start_ms;
    let mut next_ts = || {
        let ts = format_rfc3339(Timestamp(clock));
        clock = clock.saturating_add(1);
        ts
    };
    let ts = next_ts();
    lines.push(json!({
        "timestamp": ts,
        "type": "session_meta",
        "payload": {
            "id": params.session_id,
            "timestamp": ts,
            "cwd": params.cwd,
            "originator": "yulius",
            "cli_version": params.cli_version,
            "model_provider": "openai"
        }
    }));
    let mut turn_context = json!({
        "cwd": params.cwd,
        "approval_policy": "on-request",
        "sandbox_policy": { "type": "workspace-write" },
        "model": params.model,
        "summary": "auto"
    });
    if let (Some(effort), Some(object)) = (params.effort, turn_context.as_object_mut()) {
        object.insert("effort".into(), Value::String(effort.to_owned()));
    }
    lines.push(json!({ "timestamp": next_ts(), "type": "turn_context", "payload": turn_context }));

    let texts: Vec<(Role, String)> = transcript
        .turns
        .iter()
        .map(|t| (t.role, t.rendered_text()))
        .filter(|(_, text)| !text.is_empty())
        .collect();
    let message = |role: &str, kind: &str, text: &str| {
        json!({
            "type": "message",
            "role": role,
            "content": [{ "type": kind, "text": text }]
        })
    };
    let mut index = 0;
    while index < texts.len() {
        let (role, text) = &texts[index];
        let reply = texts.get(index + 1).filter(|(r, _)| *r == Role::Assistant);
        match (role, reply) {
            (Role::User, Some((_, answer))) => {
                lines.push(event(next_ts(), json!({ "type": "task_started" })));
                lines.push(item(next_ts(), message("user", "input_text", text)));
                lines.push(item(next_ts(), message("assistant", "output_text", answer)));
                lines.push(event(next_ts(), task_complete(answer)));
                index += 2;
            }
            (Role::User, None) => {
                lines.push(item(next_ts(), message("user", "input_text", text)));
                index += 1;
            }
            (Role::Assistant, _) => {
                lines.push(event(next_ts(), json!({ "type": "task_started" })));
                lines.push(item(next_ts(), message("assistant", "output_text", text)));
                lines.push(event(next_ts(), task_complete(text)));
                index += 1;
            }
        }
    }
    let mut out = String::new();
    for line in lines {
        out.push_str(&line.to_string());
        out.push('\n');
    }
    out
}

fn item(timestamp: String, payload: Value) -> Value {
    json!({ "timestamp": timestamp, "type": "response_item", "payload": payload })
}

fn event(timestamp: String, payload: Value) -> Value {
    json!({ "timestamp": timestamp, "type": "event_msg", "payload": payload })
}

fn task_complete(last_agent_message: &str) -> Value {
    json!({ "type": "task_complete", "last_agent_message": last_agent_message })
}

/// Rollout directory relative to `<CODEX_HOME>/sessions` for a start time: `YYYY/MM/DD` (UTC).
#[must_use]
pub fn rollout_relative_dir(start_ms: i64) -> PathBuf {
    let (year, month, day, ..) = civil(Timestamp(start_ms));
    PathBuf::from(format!("{year:04}"))
        .join(format!("{month:02}"))
        .join(format!("{day:02}"))
}

/// Rollout file name for a start time and session id:
/// `rollout-YYYY-MM-DDTHH-MM-SS-<session_id>.jsonl` (UTC).
#[must_use]
pub fn rollout_file_name(start_ms: i64, session_id: &str) -> String {
    let (year, month, day, hour, minute, second, _) = civil(Timestamp(start_ms));
    format!(
        "rollout-{year:04}-{month:02}-{day:02}T{hour:02}-{minute:02}-{second:02}-{session_id}.jsonl"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_summary() {
        assert_eq!(
            summarize_arguments(r#"{"command":["bash","-lc","ls"],"workdir":"/w"}"#),
            "bash -lc ls"
        );
        assert_eq!(summarize_arguments("not json"), "not json");
    }

    #[test]
    fn tool_output_forms() {
        assert_eq!(
            tool_output(Some(&json!(
                "{\"output\":\"boom\",\"metadata\":{\"exit_code\":2}}"
            ))),
            ("boom".to_owned(), true)
        );
        assert_eq!(
            tool_output(Some(&json!("plain"))),
            ("plain".to_owned(), false)
        );
        assert_eq!(
            tool_output(Some(&json!({"content": "x", "success": false}))),
            ("x".to_owned(), true)
        );
        assert_eq!(tool_output(None), (String::new(), false));
    }

    #[test]
    fn rollout_paths() {
        assert_eq!(
            rollout_relative_dir(1_791_364_320_123),
            PathBuf::from("2026/10/07")
        );
        assert_eq!(
            rollout_file_name(1_791_364_320_123, "abc"),
            "rollout-2026-10-07T09-12-00-abc.jsonl"
        );
    }
}
