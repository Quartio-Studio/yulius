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

/// Lower-case phrases that identify a usage-limit error text (older CLIs record only text).
const LIMIT_PHRASES: [&str; 5] = [
    "usage limit",
    "limit reached",
    "hit your limit",
    "session limit",
    "weekly limit",
];

/// The five-hour session window and the weekly window, in minutes.
const SESSION_WINDOW_MINUTES: u32 = 5 * 60;
const WEEKLY_WINDOW_MINUTES: u32 = 7 * 24 * 60;

/// The window of Claude's `rateLimitType` (`five_hour`, `seven_day`, `seven_day_opus`, …).
fn window_of_type(kind: &str) -> Option<u32> {
    if kind == "five_hour" {
        Some(SESSION_WINDOW_MINUTES)
    } else if kind.starts_with("seven_day") {
        Some(WEEKLY_WINDOW_MINUTES)
    } else {
        None
    }
}

/// The window a lower-case limit text names ("session limit", "5-hour limit", "weekly limit").
fn window_of_text(lower: &str) -> Option<u32> {
    if lower.contains("session limit") || lower.contains("5-hour") || lower.contains("five-hour") {
        Some(SESSION_WINDOW_MINUTES)
    } else if lower.contains("weekly") || lower.contains("7-day") {
        Some(WEEKLY_WINDOW_MINUTES)
    } else {
        None
    }
}

/// `apiError` of the error record Claude Code writes when the account's usage limit rejects a
/// request (alongside `error: "rate_limit"` and HTTP 429).
const USAGE_LIMIT_API_ERROR: &str = "usage_limit_reached";

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
        let text = message_text(message);
        return structured_limit(record, &text)
            .or_else(|| limit_signal(&text))
            .into_iter()
            .collect();
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

/// Recognizes the usage-limit error record of Claude Code 2.1.x by its structure, whatever its
/// wording ("You've hit your session limit · resets 10:20am"): `apiError: "usage_limit_reached"`
/// or a `rejected` rate-limit status, with `resetsAt` (epoch seconds) in
/// `apiErrorParams.rate_limit_info` or `quotaLimits`.
fn structured_limit(record: &Map<String, Value>, text: &str) -> Option<Observed> {
    let info = obj_of(record, "apiErrorParams")
        .and_then(|params| obj_of(params, "rate_limit_info"))
        .or_else(|| obj_of(record, "quotaLimits"));
    let rejected = info.and_then(|i| str_of(i, "status")) == Some("rejected");
    if str_of(record, "apiError") != Some(USAGE_LIMIT_API_ERROR) && !rejected {
        return None;
    }
    let resets_at = info
        .and_then(|i| i.get("resetsAt"))
        .and_then(Value::as_i64)
        .and_then(from_epoch_seconds);
    let window_minutes = info
        .and_then(|i| str_of(i, "rateLimitType"))
        .and_then(window_of_type)
        .or_else(|| window_of_text(&text.to_lowercase()));
    let message = if text.trim().is_empty() {
        "usage limit reached"
    } else {
        text.trim()
    };
    Some(Observed::LimitReached {
        resets_at,
        message: shorten(message, LIMIT_MESSAGE_BYTES),
        window_minutes,
    })
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
        window_minutes: window_of_text(&lower),
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
                message: "Claude AI usage limit reached".into(),
                window_minutes: None,
            })
        );
        assert_eq!(
            limit_signal("5-hour limit reached ∙ resets 3pm"),
            Some(Observed::LimitReached {
                resets_at: None,
                message: "5-hour limit reached ∙ resets 3pm".into(),
                window_minutes: Some(300),
            })
        );
        assert_eq!(limit_signal("API Error: 500 internal server error"), None);
        assert_eq!(
            limit_signal("You've hit your session limit · resets 10:20am (Europe/Berlin)"),
            Some(Observed::LimitReached {
                resets_at: None,
                message: "You've hit your session limit · resets 10:20am (Europe/Berlin)".into(),
                window_minutes: Some(300),
            })
        );
        assert_eq!(
            limit_signal("You've hit your weekly limit · resets Oct 12"),
            Some(Observed::LimitReached {
                resets_at: None,
                message: "You've hit your weekly limit · resets Oct 12".into(),
                window_minutes: Some(10080),
            })
        );
    }

    /// The record Claude Code 2.1.292 writes (synthetic values): structure wins over wording.
    #[test]
    fn structured_limit_record_with_reset_time() {
        let line = r#"{"type":"assistant","isApiErrorMessage":true,"error":"rate_limit","apiError":"usage_limit_reached","apiErrorStatus":429,"apiErrorParams":{"rate_limit_info":{"status":"rejected","resetsAt":1760000000,"rateLimitType":"five_hour"}},"quotaLimits":{"status":"rejected","resetsAt":1760000000},"message":{"model":"<synthetic>","id":"m","type":"message","role":"assistant","content":[{"type":"text","text":"Limit hit, see you soon"}],"stop_reason":"stop_sequence"}}"#;
        assert_eq!(
            observe_line(line),
            vec![Observed::LimitReached {
                resets_at: Some(Timestamp(1_760_000_000_000)),
                message: "Limit hit, see you soon".into(),
                window_minutes: Some(300),
            }]
        );
        // Only `quotaLimits` (older 2.1 builds) still carries the reset time.
        let quota = r#"{"type":"assistant","isApiErrorMessage":true,"error":"rate_limit","quotaLimits":{"status":"rejected","resetsAt":1760000000},"message":{"model":"<synthetic>","content":[]}}"#;
        assert_eq!(
            observe_line(quota),
            vec![Observed::LimitReached {
                resets_at: Some(Timestamp(1_760_000_000_000)),
                message: "usage limit reached".into(),
                window_minutes: None,
            }]
        );
        // A transient rate limit that was retried is not a usage limit.
        let transient = r#"{"type":"assistant","isApiErrorMessage":true,"error":"rate_limit","quotaLimits":{"status":"allowed_warning","resetsAt":1760000000},"message":{"model":"<synthetic>","content":[{"type":"text","text":"API Error: Request rejected (429) · retrying"}]}}"#;
        assert_eq!(observe_line(transient), Vec::<Observed>::new());
    }
}
