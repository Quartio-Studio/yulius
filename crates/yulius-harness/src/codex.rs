//! Codex rollout observation (structure of Codex CLI 0.167.0 and 0.169.0; SPEC-503 CDX-004).
//!
//! A rollout is `<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-<local time>-<thread id>.jsonl`, created
//! at the session's first turn. Each line is `{timestamp, ordinal?, type, payload}`; the
//! interactive TUI writes "paginated" history, which numbers lines (`ordinal`) and records
//! messages as `event_msg` `item_completed` besides the `response_item` messages. Types used here:
//! `session_meta`, `turn_context`, `response_item`, `event_msg`, `token_usage_record` and
//! `compacted`; the others (`world_state`, …) are ignored, and malformed lines are reported, never
//! fatal. A response's tokens are its `token_usage_record` (one per response, keyed by its
//! `response_id`): `token_count` repeats the session's last usage whenever Codex reports again
//! (a resume that meets a usage limit, a compaction), so it gives only the rate limits.

use serde_json::{Map, Value};
use yulius_domain::time::{Timestamp, parse_rfc3339};
use yulius_domain::usage::{AccountUsage, LimitScope, TokenUsage, UsageWindow};

use crate::json::{obj_of, owned_str, parse_object, shorten, str_of, u64_of};
use crate::observe::Observed;
use crate::time::from_epoch_seconds;

/// `task_complete.error.codex_error_info` of a turn a usage limit ended (0.167).
const USAGE_LIMIT_ERROR: &str = "usage_limit_exceeded";

/// Record types this module reads; any other type is ignored.
const KNOWN_TYPES: [&str; 6] = [
    "session_meta",
    "turn_context",
    "response_item",
    "event_msg",
    "token_usage_record",
    "compacted",
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
    if !KNOWN_TYPES.contains(&kind) {
        return Vec::new();
    }
    let at = str_of(&record, "timestamp").and_then(parse_rfc3339);
    let Some(payload) = obj_of(&record, "payload") else {
        return vec![Observed::Malformed];
    };
    match kind {
        "session_meta" => match owned_str(payload, "id") {
            // Its `cli_version` is Codex's, which no part of Yulius reads as a Claude Code
            // version (SPEC-461 keeps one per run).
            Some(session_id) => vec![Observed::SessionMeta {
                session_id,
                cwd: owned_str(payload, "cwd"),
                model: None,
                version: None,
            }],
            None => vec![Observed::Malformed],
        },
        "turn_context" => vec![Observed::TurnContext {
            model: owned_str(payload, "model"),
        }],
        "response_item" => observe_item(payload, at),
        "token_usage_record" => vec![response_usage(payload, None).unwrap_or(Observed::Malformed)],
        "compacted" => vec![Observed::Compacted],
        _ => observe_event(payload, at),
    }
}

/// The response a `token_usage_record` payload counts, answered by `model`; `None` without a
/// response id or usage. The response id is its key: a line read twice, one a resumed run reads
/// again or one a subagent's rollout repeats counts once.
pub(crate) fn response_usage(
    payload: &Map<String, Value>,
    model: Option<String>,
) -> Option<Observed> {
    let response = str_of(payload, "response_id")?;
    let usage = obj_of(payload, "usage")?;
    Some(Observed::AssistantMessage {
        message_id: format!("codex-{response}"),
        model,
        usage: token_usage(usage),
    })
}

/// When the session of a rollout started, from its first line (`session_meta`): the payload's
/// `timestamp`, else the line's. `None` for any other line.
#[must_use]
pub fn session_started_at(line: &str) -> Option<Timestamp> {
    let record = parse_object(line)?;
    if str_of(&record, "type") != Some("session_meta") {
        return None;
    }
    obj_of(&record, "payload")
        .and_then(|payload| str_of(payload, "timestamp"))
        .or_else(|| str_of(&record, "timestamp"))
        .and_then(parse_rfc3339)
}

fn observe_item(payload: &Map<String, Value>, at: Option<Timestamp>) -> Vec<Observed> {
    let is_message = str_of(payload, "type") == Some("message");
    match str_of(payload, "role") {
        Some("user") if is_message => {
            let typed = typed_by_provenance(payload).unwrap_or_else(|| {
                message_texts(payload)
                    .iter()
                    .any(|text| !is_injected_context(text))
            });
            if typed {
                vec![Observed::UserTurn { at, wake: false }]
            } else {
                Vec::new()
            }
        }
        // Developer and system instructions are no one's work.
        Some("developer" | "system") if is_message => Vec::new(),
        // The agent's answer, reasoning and tool calls: the turn goes on.
        _ => vec![Observed::AgentWork { at }],
    }
}

fn observe_event(payload: &Map<String, Value>, at: Option<Timestamp>) -> Vec<Observed> {
    match str_of(payload, "type") {
        // The account's windows; a reached one marks the account limited. The turn's end says
        // whether the limit cut it (below). Its token figures are the session's, counted from
        // each response's `token_usage_record` instead.
        Some("token_count") => obj_of(payload, "rate_limits")
            .map(|limits| vec![Observed::RateLimits(account_usage(limits, at))])
            .unwrap_or_default(),
        Some("task_complete") => match obj_of(payload, "error") {
            // A usage limit ends the turn with an error; Codex does not continue it by itself.
            // Its reset time is only local-time text here: the rate limits give the exact one.
            Some(error) if str_of(error, "codex_error_info") == Some(USAGE_LIMIT_ERROR) => {
                vec![Observed::LimitReached {
                    resets_at: None,
                    message: shorten(
                        str_of(error, "message").unwrap_or("Codex usage limit reached"),
                        256,
                    ),
                    window_minutes: None,
                    scope: LimitScope::Account,
                    at,
                }]
            }
            _ => vec![Observed::TurnComplete {
                duration_ms: u64_of(payload, "duration_ms"),
                at,
            }],
        },
        Some("turn_aborted") => vec![Observed::TurnInterrupted { at }],
        _ => Vec::new(),
    }
}

/// Codex counts cached and newly cached tokens inside `input_tokens`; Yulius's `input` is the
/// input that is neither.
fn token_usage(usage: &Map<String, Value>) -> TokenUsage {
    let input = u64_of(usage, "input_tokens");
    let cached = u64_of(usage, "cached_input_tokens");
    let written = u64_of(usage, "cache_write_input_tokens");
    TokenUsage {
        input: input.map(|i| {
            i.saturating_sub(cached.unwrap_or(0))
                .saturating_sub(written.unwrap_or(0))
        }),
        output: u64_of(usage, "output_tokens"),
        cache_read: cached,
        cache_write: written,
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

/// The used-up window of a reached limit: the full one that resets last.
fn full_window(usage: &AccountUsage) -> Option<UsageWindow> {
    usage
        .primary
        .iter()
        .chain(usage.secondary.iter())
        .filter(|w| w.used_percent.is_some_and(|p| p >= 100.0))
        .max_by_key(|w| w.resets_at)
        .copied()
}

fn account_usage(limits: &Map<String, Value>, at: Option<Timestamp>) -> AccountUsage {
    let primary = window(limits.get("primary"));
    let secondary = window(limits.get("secondary"));
    // `null` while no limit is reached (0.167 names the kind of limit otherwise).
    let limit_reached = str_of(limits, "rate_limit_reached_type").is_some_and(|t| !t.is_empty());
    let mut usage = AccountUsage {
        primary,
        secondary,
        limit_reached,
        observed_at: at,
        ..AccountUsage::default()
    };
    if limit_reached {
        // Without a full window, wait for the latest known reset (conservative).
        usage.limit_resets_at = full_window(&usage).and_then(|w| w.resets_at).or_else(|| {
            primary
                .iter()
                .chain(secondary.iter())
                .filter_map(|w| w.resets_at)
                .max()
        });
    }
    usage
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

/// Whether a user-role message was typed, as Codex 0.169 marks it: each content item's
/// `provenance.type` is `user` for what a person typed and `harness` for what Codex added (its
/// environment context, a blocking hook's continuation). `None` without that metadata, and when
/// every item is `unknown`, which may be either (`ContentItemProvenance::Unknown` in
/// `protocol/src/models/item_metadata.rs`: history saved before attribution, or content whose
/// source is not known): the text decides then.
fn typed_by_provenance(payload: &Map<String, Value>) -> Option<bool> {
    let items = obj_of(payload, "internal_chat_message_metadata_passthrough")?
        .get("content_item_metadata")?
        .as_array()?;
    let kinds: Vec<&str> = items
        .iter()
        .filter_map(|item| item.get("provenance")?.get("type")?.as_str())
        .filter(|kind| *kind != "unknown")
        .collect();
    (!kinds.is_empty()).then(|| kinds.contains(&"user"))
}

fn is_injected_context(text: &str) -> bool {
    yulius_domain::harness::is_codex_injected_context(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Codex marks what a person typed `user` and what it added `harness`; an item it marks
    /// `unknown` may be either (`ContentItemProvenance::Unknown`: history saved before
    /// attribution, or content whose source is not known), so the text decides as for a message
    /// without provenance.
    #[test]
    fn unknown_provenance_lets_the_text_decide() {
        let message = |kind: &str, text: &str| {
            serde_json::json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": text}],
                "internal_chat_message_metadata_passthrough": {
                    "content_item_metadata": [{"provenance": {"type": kind}}]
                }
            })
        };
        let typed = |value: serde_json::Value| {
            let line = serde_json::json!({
                "timestamp": "2026-11-16T20:53:49.000Z",
                "type": "response_item",
                "payload": value
            });
            observe_line(&line.to_string())
                .iter()
                .any(|o| matches!(o, Observed::UserTurn { .. }))
        };
        assert!(typed(message("unknown", "Fix the failing test")));
        assert!(!typed(message(
            "unknown",
            "<environment_context><cwd>/w</cwd>"
        )));
        assert!(typed(message(
            "user",
            "<environment_context> typed by a person"
        )));
        assert!(!typed(message("harness", "Run the gate first.")));
    }

    #[test]
    fn injected_context_is_not_a_prompt() {
        assert!(is_injected_context(
            "  <environment_context>\n<cwd>/w</cwd>"
        ));
        assert!(is_injected_context("# AGENTS.md instructions for /w"));
        assert!(is_injected_context(
            "<hook_prompt hook_run_id=\"stop:0\">Run the gate"
        ));
        assert!(!is_injected_context("Fix the failing test"));
    }
}
