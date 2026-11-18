//! Antigravity transcript observation (structure of the Antigravity CLI 1.5.10; SPEC-505
//! AGY-004).
//!
//! A conversation's transcript is `<HOME>/.gemini/antigravity-cli/brain/<conversation>/
//! .system_generated/logs/transcript.jsonl`: one JSON object per step of the conversation, with
//! `step_index`, `source` (`USER_EXPLICIT`, `MODEL`, `SYSTEM`), `type` (`USER_INPUT`,
//! `PLANNER_RESPONSE`, `SYSTEM_MESSAGE`, `ERROR_MESSAGE`, `GENERIC`, …), `status` and
//! `created_at`; a model response adds `model`, `input_tokens` (without the cached ones),
//! `cache_read_tokens`, `output_tokens` (thinking included) and `tool_calls`; an error adds
//! `error`. Steps of types not read here are the agent's work when the model made them and no
//! one's otherwise; malformed lines are reported, never fatal.

use serde_json::{Map, Value};
use yulius_domain::time::{Timestamp, parse_rfc3339};
use yulius_domain::usage::{LimitScope, TokenUsage};

use crate::json::{owned_str, parse_object, shorten, str_of, u64_of};
use crate::observe::Observed;

/// Texts of an error step that a plan's quota, a spend cap or a prepaid balance wrote: the
/// subscription is used up (lower-case; the CLI's own words, from its 1.5.10 release notes and
/// the Gemini API). The CLI may try a few times more; each attempt's step is the same limit.
const LIMIT_TEXTS: [&str; 4] = [
    "quota reached",
    "exceeded your current quota",
    "credits balance is too low",
    "out of quota",
];

/// The Gemini API's refusal of a request (HTTP 429), lower-case.
const REFUSAL_TEXTS: [&str; 2] = ["resource_exhausted", "error 429"];

/// When a transcript step was recorded (its `created_at`, to the second), if it says.
#[must_use]
pub fn record_time(line: &str) -> Option<Timestamp> {
    parse_object(line)
        .as_ref()
        .and_then(|step| str_of(step, "created_at"))
        .and_then(parse_rfc3339)
}

/// Normalizes one Antigravity transcript line.
#[must_use]
pub fn observe_line(line: &str) -> Vec<Observed> {
    let Some(step) = parse_object(line) else {
        return vec![Observed::Malformed];
    };
    let (Some(kind), Some(source)) = (str_of(&step, "type"), str_of(&step, "source")) else {
        return vec![Observed::Malformed];
    };
    let at = str_of(&step, "created_at").and_then(parse_rfc3339);
    match (source, kind) {
        // What a person (or Yulius, typing for one) sent.
        ("USER_EXPLICIT", "USER_INPUT") => vec![Observed::UserTurn { at, wake: false }],
        ("MODEL", "PLANNER_RESPONSE") => model_response(&step, at),
        // A model API error, one step per attempt agy makes: a limit of the subscription, or
        // nothing (agy goes on; its Stop ends a turn that failed for good).
        ("SYSTEM", "ERROR_MESSAGE") => limit(&step, at).into_iter().collect(),
        // A prompt Yulius refused (its hook's error): the turn ends as an interrupt's does, with
        // no Stop Yulius answers.
        ("SYSTEM_SDK", "ERROR_MESSAGE") => vec![Observed::TurnInterrupted { at }],
        // Tool steps, refused tool calls, anything the model did.
        ("MODEL", _) => vec![Observed::AgentWork { at }],
        // System messages and the CLI's own notices are no one's work.
        _ => Vec::new(),
    }
}

/// A model response: its model, its tokens (keyed by the step, so a line read twice or again by
/// a resumed run counts once), and the agent's work.
fn model_response(step: &Map<String, Value>, at: Option<Timestamp>) -> Vec<Observed> {
    let model = owned_str(step, "model");
    let key = format!(
        "agy-{}-{}",
        u64_of(step, "step_index").map_or_else(String::new, |i| i.to_string()),
        str_of(step, "created_at").unwrap_or_default()
    );
    vec![
        Observed::TurnContext {
            model: model.clone(),
        },
        Observed::AssistantMessage {
            message_id: key,
            model,
            usage: TokenUsage {
                input: u64_of(step, "input_tokens"),
                output: u64_of(step, "output_tokens"),
                cache_read: u64_of(step, "cache_read_tokens"),
                cache_write: None,
            },
        },
        Observed::AgentWork { at },
    ]
}

/// An error step that says the subscription's quota is used up: a plan's or the API's quota
/// text, or a 429 that names the quota it used up (`quotaId`). A 429 naming none is the model's
/// capacity, which the CLI retries by itself, and a per-minute quota it waits out: neither is a
/// limit of the subscription. The CLI writes one step per attempt. Its reset time is not given in
/// a form Yulius reads.
fn limit(step: &Map<String, Value>, at: Option<Timestamp>) -> Option<Observed> {
    let error = str_of(step, "error")?;
    let lower = error.to_ascii_lowercase();
    let squeezed: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
    let limited = !squeezed.contains("perminute")
        && (LIMIT_TEXTS.iter().any(|text| lower.contains(text))
            || (REFUSAL_TEXTS.iter().any(|text| lower.contains(text))
                && squeezed.contains("quotaid")));
    limited.then(|| Observed::LimitReached {
        resets_at: None,
        message: shorten(error, 256),
        window_minutes: None,
        scope: LimitScope::Account,
        at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Error 429" alone is a limit; a 503 or a per-minute 429 is not.
    #[test]
    fn which_errors_are_limits() {
        let step = |error: &str| {
            let mut map = Map::new();
            map.insert("error".into(), Value::String(error.into()));
            map
        };
        assert!(limit(&step("Error 429, Message: Too many"), None).is_none());
        assert!(limit(&step("Error 429 quotaId:XPerDay"), None).is_some());
        assert!(limit(&step("Error 503, Status: UNAVAILABLE"), None).is_none());
        assert!(limit(&step("Error 429 quotaId:X Per Minute"), None).is_none());
        assert!(limit(&Map::new(), None).is_none());
    }
}
