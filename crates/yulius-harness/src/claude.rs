//! Claude Code JSONL transcripts (structure of Claude Code 2.1.292): observation of turns, usage
//! and limits (SPEC-002 §7.2).
//!
//! Records are JSON objects with a `type`. `user`/`assistant`/`system` records carry
//! `parentUuid`, `isSidechain`, `uuid`, `timestamp`, `userType`, `cwd`, `sessionId`, `version`
//! and `gitBranch`. Claude writes one line per content block of an API message, each repeating
//! `message.id` and `message.usage`.

use serde_json::{Map, Value};
use yulius_domain::usage::TokenUsage;

use crate::json::{bool_of, obj_of, owned_str, parse_object, shorten, str_of, u64_of};
use crate::observe::Observed;
use crate::time::from_epoch_seconds;

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
#[cfg(test)]
mod tests {
    use yulius_domain::time::Timestamp;

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
}
