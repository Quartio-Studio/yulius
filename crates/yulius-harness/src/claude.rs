//! Claude Code JSONL transcripts (structure of Claude Code 2.1.292): observation, reading into
//! the IR and writing a resumable native transcript (SPEC-002 §7.2, §7.3).
//!
//! Records are JSON objects with a `type`. `user`/`assistant`/`system` records carry
//! `parentUuid`, `isSidechain`, `uuid`, `timestamp`, `userType`, `cwd`, `sessionId`, `version`
//! and `gitBranch`. Claude writes one line per content block of an API message, each repeating
//! `message.id` and `message.usage`.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value, json};
use yulius_domain::harness::Harness;
use yulius_domain::time::Timestamp;
use yulius_domain::transcript::{Block, Role, Transcript, TranscriptMeta, Turn};
use yulius_domain::usage::TokenUsage;

use crate::ir::push_blocks;
use crate::json::{
    SUMMARY_READ_BYTES, bool_of, compact, obj_of, owned_str, parse_object, shorten, str_of,
    timestamp_of, u64_of,
};
use crate::observe::Observed;
use crate::time::{format_rfc3339, from_epoch_seconds};

/// Maximum bytes of provider limit text kept in [`Observed::LimitReached`].
const LIMIT_MESSAGE_BYTES: usize = 512;

/// Model name Claude uses for locally generated (non-API) messages.
const SYNTHETIC_MODEL: &str = "<synthetic>";

/// Lower-case phrases that identify a usage-limit error text.
const LIMIT_PHRASES: [&str; 3] = ["usage limit", "limit reached", "hit your limit"];

/// Normalizes one Claude Code transcript line.
#[must_use]
pub fn observe_line(line: &str) -> Vec<Observed> {
    let Some(record) = parse_object(line) else {
        return vec![Observed::Malformed];
    };
    match str_of(&record, "type") {
        Some("user") => observe_user(&record),
        Some("assistant") => observe_assistant(&record),
        Some(_) => Vec::new(),
        None => vec![Observed::Malformed],
    }
}

fn observe_user(record: &Map<String, Value>) -> Vec<Observed> {
    let Some(message) = obj_of(record, "message") else {
        return vec![Observed::Malformed];
    };
    if bool_of(record, "isSidechain") || bool_of(record, "isMeta") || !is_prompt(message) {
        return Vec::new();
    }
    let Some(session_id) = owned_str(record, "sessionId") else {
        return vec![Observed::UserTurn];
    };
    vec![
        Observed::SessionMeta {
            session_id,
            cwd: owned_str(record, "cwd"),
            model: None,
            version: owned_str(record, "version"),
        },
        Observed::UserTurn,
    ]
}

/// A user message is a prompt when it has text (string content or a text block), not only
/// tool results.
fn is_prompt(message: &Map<String, Value>) -> bool {
    match message.get("content") {
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .any(|b| b.get("type").and_then(Value::as_str) == Some("text")),
        _ => false,
    }
}

fn observe_assistant(record: &Map<String, Value>) -> Vec<Observed> {
    let Some(message) = obj_of(record, "message") else {
        return vec![Observed::Malformed];
    };
    let is_error =
        bool_of(record, "isApiErrorMessage") || record.get("error").is_some_and(|e| !e.is_null());
    if is_error {
        return limit_signal(&message_text(message)).into_iter().collect();
    }
    let model = owned_str(message, "model").filter(|m| m != SYNTHETIC_MODEL);
    let mut out = Vec::new();
    if let Some(message_id) = owned_str(message, "id") {
        out.push(Observed::AssistantMessage {
            message_id,
            model,
            usage: obj_of(message, "usage").map_or(TokenUsage::UNKNOWN, usage_of),
        });
    }
    if str_of(message, "stop_reason") == Some("end_turn") {
        out.push(Observed::TurnComplete { duration_ms: None });
    }
    out
}

fn usage_of(usage: &Map<String, Value>) -> TokenUsage {
    TokenUsage {
        input: u64_of(usage, "input_tokens"),
        output: u64_of(usage, "output_tokens"),
        cache_read: u64_of(usage, "cache_read_input_tokens"),
        cache_write: u64_of(usage, "cache_creation_input_tokens"),
    }
}

/// Text of a message: string content or its text blocks joined by newlines.
fn message_text(message: &Map<String, Value>) -> String {
    match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Recognizes Claude usage-limit error text such as `Claude AI usage limit reached|1760000000`
/// (epoch seconds after `|`) or `5-hour limit reached ∙ resets 3pm` (no absolute time).
fn limit_signal(text: &str) -> Option<Observed> {
    let lower = text.to_lowercase();
    if !LIMIT_PHRASES.iter().any(|p| lower.contains(p)) {
        return None;
    }
    let (human, resets_at) = match text.split_once('|') {
        Some((human, rest)) => {
            let digits: String = rest
                .trim_start()
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            let resets_at = digits.parse::<i64>().ok().and_then(from_epoch_seconds);
            (human, resets_at)
        }
        None => (text, None),
    };
    Some(Observed::LimitReached {
        resets_at,
        message: shorten(human.trim(), LIMIT_MESSAGE_BYTES),
    })
}

/// Reads a Claude Code transcript into the IR.
///
/// Follows the `parentUuid` chain back from the last main-chain user/assistant record, so
/// abandoned branches (rewinds) are excluded; falls back to file order when records lack
/// `uuid`. Sidechain, meta and synthetic error records are skipped. Tool results join the
/// preceding assistant turn; thinking and images become [`Block::Omitted`]. Malformed lines
/// and unknown record types are ignored.
#[must_use]
pub fn read_transcript<I, S>(lines: I) -> Transcript
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let records: Vec<Map<String, Value>> = lines
        .into_iter()
        .filter_map(|line| parse_object(line.as_ref()))
        .collect();
    let mut meta = TranscriptMeta {
        source: Harness::ClaudeCode,
        session_id: String::new(),
        cwd: None,
        model: None,
    };
    let mut turns: Vec<Turn> = Vec::new();
    for record in active_chain(&records) {
        if meta.session_id.is_empty()
            && let Some(id) = str_of(record, "sessionId")
        {
            id.clone_into(&mut meta.session_id);
        }
        if meta.cwd.is_none() {
            meta.cwd = owned_str(record, "cwd");
        }
        let Some(message) = obj_of(record, "message") else {
            continue;
        };
        let at = timestamp_of(record, "timestamp");
        match str_of(record, "type") {
            Some("user") => read_user(message, at, &mut turns),
            Some("assistant") if !bool_of(record, "isApiErrorMessage") => {
                if meta.model.is_none() {
                    meta.model = owned_str(message, "model").filter(|m| m != SYNTHETIC_MODEL);
                }
                let blocks = assistant_blocks(message);
                push_blocks(&mut turns, Role::Assistant, blocks, at);
            }
            _ => {}
        }
    }
    Transcript { meta, turns }
}

/// Main-chain user/assistant records in conversation order.
fn active_chain(records: &[Map<String, Value>]) -> Vec<&Map<String, Value>> {
    let conversational = |r: &Map<String, Value>| {
        matches!(str_of(r, "type"), Some("user" | "assistant"))
            && !bool_of(r, "isSidechain")
            && !bool_of(r, "isMeta")
    };
    let linear = || records.iter().filter(|r| conversational(r)).collect();
    let Some(leaf) = records.iter().rev().find(|r| conversational(r)) else {
        return Vec::new();
    };
    let Some(leaf_uuid) = str_of(leaf, "uuid") else {
        return linear();
    };
    let mut by_uuid: HashMap<&str, &Map<String, Value>> = HashMap::new();
    for record in records {
        if let Some(uuid) = str_of(record, "uuid") {
            by_uuid.entry(uuid).or_insert(record);
        }
    }
    let mut chain = Vec::new();
    let mut visited = HashSet::new();
    let mut cursor = Some(leaf_uuid);
    while let Some(uuid) = cursor {
        if !visited.insert(uuid) {
            break; // cycle guard
        }
        let Some(record) = by_uuid.get(uuid) else {
            break;
        };
        chain.push(*record);
        cursor = str_of(record, "parentUuid");
    }
    chain.reverse();
    chain.into_iter().filter(|r| conversational(r)).collect()
}

fn read_user(message: &Map<String, Value>, at: Option<Timestamp>, turns: &mut Vec<Turn>) {
    match message.get("content") {
        Some(Value::String(text)) => {
            push_blocks(turns, Role::User, vec![Block::Text(text.clone())], at);
        }
        Some(Value::Array(items)) => {
            for item in items {
                let Some(block) = item.as_object() else {
                    continue;
                };
                match str_of(block, "type") {
                    Some("text") => {
                        let text = str_of(block, "text").unwrap_or_default().to_owned();
                        push_blocks(turns, Role::User, vec![Block::Text(text)], at);
                    }
                    Some("tool_result") => {
                        let result = Block::ToolResult {
                            summary: shorten(&tool_result_text(block), SUMMARY_READ_BYTES),
                            is_error: bool_of(block, "is_error"),
                        };
                        push_blocks(turns, Role::Assistant, vec![result], at);
                    }
                    Some("image") => {
                        let omitted = Block::Omitted {
                            what: "image".into(),
                        };
                        push_blocks(turns, Role::User, vec![omitted], at);
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn tool_result_text(block: &Map<String, Value>) -> String {
    match block.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| match p.get("type").and_then(Value::as_str) {
                Some("text") => p.get("text").and_then(Value::as_str).map(str::to_owned),
                Some("image") => Some("[image omitted]".to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) if !other.is_null() => compact(other, SUMMARY_READ_BYTES),
        _ => String::new(),
    }
}

fn assistant_blocks(message: &Map<String, Value>) -> Vec<Block> {
    let Some(Value::Array(items)) = message.get("content") else {
        return match message.get("content") {
            Some(Value::String(text)) => vec![Block::Text(text.clone())],
            _ => Vec::new(),
        };
    };
    items
        .iter()
        .filter_map(Value::as_object)
        .filter_map(|block| match str_of(block, "type") {
            Some("text") => Some(Block::Text(
                str_of(block, "text").unwrap_or_default().to_owned(),
            )),
            Some("tool_use") => Some(Block::ToolCall {
                name: str_of(block, "name").unwrap_or("tool").to_owned(),
                input_summary: block
                    .get("input")
                    .map_or_else(String::new, summarize_tool_input),
            }),
            Some("thinking" | "redacted_thinking") => Some(Block::Omitted {
                what: "thinking".into(),
            }),
            Some("image") => Some(Block::Omitted {
                what: "image".into(),
            }),
            _ => None,
        })
        .collect()
}

/// Short rendering of tool arguments: the command or path when present, else compact JSON.
fn summarize_tool_input(input: &Value) -> String {
    for key in ["command", "file_path", "path", "pattern", "url", "query"] {
        if let Some(text) = input.get(key).and_then(Value::as_str) {
            return shorten(text, SUMMARY_READ_BYTES);
        }
    }
    compact(input, SUMMARY_READ_BYTES)
}

/// Parameters of [`write_transcript`].
pub struct WriteParams<'a> {
    /// Session id of the synthesized transcript (file name stem and `sessionId`).
    pub session_id: &'a str,
    /// Working directory recorded in every record (the task worktree).
    pub cwd: &'a str,
    /// Claude Code version recorded in every record.
    pub version: &'a str,
    /// Model recorded on assistant messages (the model the resumed session will use).
    pub model: &'a str,
    /// Git branch recorded in every record.
    pub git_branch: &'a str,
    /// Timestamp of the first record in milliseconds since the epoch; each later record is
    /// one millisecond later.
    pub start_ms: i64,
    /// Generator of fresh record uuids (and assistant message id suffixes).
    pub ids: &'a mut dyn FnMut() -> String,
}

/// Writes a Claude Code transcript that `claude --resume <session_id>` can load.
///
/// One record per turn: a `user` or `assistant` record linked by `parentUuid`, with
/// `isSidechain: false`, `userType: "external"` and a single text content block (non-text IR
/// blocks are rendered as text). Assistant messages report zero usage and `end_turn`. Empty
/// turns are skipped. The result is JSONL with a trailing newline.
#[must_use]
pub fn write_transcript(transcript: &Transcript, params: WriteParams<'_>) -> String {
    let mut out = String::new();
    let mut parent: Option<String> = None;
    let mut index: i64 = 0;
    for turn in &transcript.turns {
        let text = turn.rendered_text();
        if text.is_empty() {
            continue;
        }
        let uuid = (params.ids)();
        let timestamp = format_rfc3339(Timestamp(params.start_ms.saturating_add(index)));
        index += 1;
        let content = json!([{ "type": "text", "text": text }]);
        let (kind, message) = match turn.role {
            Role::User => ("user", json!({ "role": "user", "content": content })),
            Role::Assistant => {
                let message_id = format!("msg_yulius_{}", (params.ids)());
                (
                    "assistant",
                    json!({
                        "id": message_id,
                        "type": "message",
                        "role": "assistant",
                        "model": params.model,
                        "content": content,
                        "stop_reason": "end_turn",
                        "stop_sequence": null,
                        "usage": {
                            "input_tokens": 0,
                            "output_tokens": 0,
                            "cache_creation_input_tokens": 0,
                            "cache_read_input_tokens": 0
                        }
                    }),
                )
            }
        };
        let record = json!({
            "parentUuid": parent,
            "isSidechain": false,
            "userType": "external",
            "cwd": params.cwd,
            "sessionId": params.session_id,
            "version": params.version,
            "gitBranch": params.git_branch,
            "type": kind,
            "message": message,
            "uuid": uuid,
            "timestamp": timestamp,
        });
        out.push_str(&record.to_string());
        out.push('\n');
        parent = Some(uuid);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_text_with_and_without_epoch() {
        assert_eq!(
            limit_signal("Claude AI usage limit reached|1760000000"),
            Some(Observed::LimitReached {
                resets_at: Some(Timestamp(1_760_000_000_000)),
                message: "Claude AI usage limit reached".into()
            })
        );
        assert_eq!(
            limit_signal("5-hour limit reached ∙ resets 3pm"),
            Some(Observed::LimitReached {
                resets_at: None,
                message: "5-hour limit reached ∙ resets 3pm".into()
            })
        );
        assert_eq!(limit_signal("API Error: 500 internal server error"), None);
    }

    #[test]
    fn tool_input_summary_prefers_command() {
        assert_eq!(
            summarize_tool_input(&json!({"command": "ls -la", "description": "list"})),
            "ls -la"
        );
        assert_eq!(summarize_tool_input(&json!({"a": 1})), "{\"a\":1}");
    }
}
