//! Codex rollout observation (structure of Codex CLI 0.167.0; SPEC-451 CDX-004).
//!
//! A rollout is `<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-<local time>-<thread id>.jsonl`, created
//! at the session's first turn. Each line is `{timestamp, ordinal?, type, payload}`; the
//! interactive TUI writes "paginated" history, which numbers lines (`ordinal`) and records
//! messages as `event_msg` `item_completed` besides the `response_item` messages. Types used here:
//! `session_meta`, `turn_context`, `response_item`, `event_msg` and `compacted`; the others
//! (`token_usage_record`, `world_state`, …) are ignored, and malformed lines are reported, never
//! fatal.

use serde_json::{Map, Value};
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, TokenUsage, UsageWindow};

use crate::json::{obj_of, owned_str, parse_object, shorten, str_of, u64_of};
use crate::observe::Observed;
use crate::time::{from_epoch_seconds, parse_rfc3339};

/// Prefixes of user-role messages that Codex injects as context rather than typed prompts.
const INJECTED_CONTEXT_PREFIXES: [&str; 6] = [
    "<environment_context",
    "<user_instructions",
    "# AGENTS.md instructions",
    "<turn_aborted",
    "<user_shell_command",
    "<permissions instructions",
];

/// `task_complete.error.codex_error_info` of a turn a usage limit ended (0.167).
const USAGE_LIMIT_ERROR: &str = "usage_limit_exceeded";

/// Record types this module reads; any other type is ignored.
const KNOWN_TYPES: [&str; 5] = [
    "session_meta",
    "turn_context",
    "response_item",
    "event_msg",
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
            Some(session_id) => vec![Observed::SessionMeta {
                session_id,
                cwd: owned_str(payload, "cwd"),
                model: None,
                version: owned_str(payload, "cli_version"),
            }],
            None => vec![Observed::Malformed],
        },
        "turn_context" => vec![Observed::TurnContext {
            model: owned_str(payload, "model"),
        }],
        "response_item" => observe_item(payload, at),
        "compacted" => vec![Observed::Compacted],
        _ => observe_event(payload, at),
    }
}

fn observe_item(payload: &Map<String, Value>, at: Option<Timestamp>) -> Vec<Observed> {
    let is_message = str_of(payload, "type") == Some("message");
    match str_of(payload, "role") {
        Some("user") if is_message => {
            let typed = message_texts(payload)
                .iter()
                .any(|text| !is_injected_context(text));
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
        Some("token_count") => {
            let mut out = Vec::new();
            if let Some(info) = obj_of(payload, "info")
                && let Some(last) = obj_of(info, "last_token_usage")
            {
                // `total_tokens` of the session so far grows with every response: the response's
                // key, so a line read twice counts once and a resumed run counts only its own.
                let total = obj_of(info, "total_token_usage")
                    .and_then(|t| u64_of(t, "total_tokens"))
                    .unwrap_or_default();
                out.push(Observed::AssistantMessage {
                    message_id: format!("codex-{total}"),
                    model: None,
                    usage: token_usage(last),
                });
            }
            // The account's windows; a reached one marks the account limited. The turn's end
            // says whether the limit cut it (below).
            if let Some(limits) = obj_of(payload, "rate_limits") {
                out.push(Observed::RateLimits(account_usage(limits, at)));
            }
            out
        }
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

/// Codex counts cached tokens inside `input_tokens`; Yulius's `input` is uncached input.
fn token_usage(usage: &Map<String, Value>) -> TokenUsage {
    let input = u64_of(usage, "input_tokens");
    let cached = u64_of(usage, "cached_input_tokens");
    TokenUsage {
        input: match (input, cached) {
            (Some(i), Some(c)) => Some(i.saturating_sub(c)),
            (i, _) => i,
        },
        output: u64_of(usage, "output_tokens"),
        cache_read: cached,
        cache_write: u64_of(usage, "cache_write_input_tokens"),
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

fn is_injected_context(text: &str) -> bool {
    let trimmed = text.trim_start();
    INJECTED_CONTEXT_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_context_is_not_a_prompt() {
        assert!(is_injected_context(
            "  <environment_context>\n<cwd>/w</cwd>"
        ));
        assert!(is_injected_context("# AGENTS.md instructions for /w"));
        assert!(!is_injected_context("Fix the failing test"));
    }
}
