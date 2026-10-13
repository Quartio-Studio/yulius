//! Claude Code JSONL transcripts (structure of Claude Code 2.1.292): observation of turns, usage
//! and limits (SPEC-002 §7.2).
//!
//! Records are JSON objects with a `type`. `user`/`assistant`/`system`/`attachment` records carry
//! `parentUuid`, `isSidechain`, `uuid`, `timestamp`, `userType`, `cwd`, `sessionId`, `version`
//! and `gitBranch`. Claude writes one line per content block of an API message, each repeating
//! `message.id` and `message.usage`.
//!
//! Goal mode (`/goal`, TSK-010/TSK-014) is recorded as `attachment` records whose
//! `attachment.type` is `goal_status`: `{"met":false,"sentinel":true,"condition":…}` while a goal
//! is active (a sentinel with `met:true` marks a goal cleared without being achieved),
//! `{"met":false,"condition":…,"reason":…}` when the evaluator keeps the agent working,
//! `{"met":true,…,"iterations":n,"durationMs":…,"tokens":…}` when it is achieved ("✓ Goal
//! achieved") and `{"met":false,"failed":true,…}` when it cannot be. The evaluator is a Stop hook,
//! so the record follows the Stop hooks of the turn, before the `system` records
//! `stop_hook_summary` and `turn_duration` ("Brewed for 51s") that close it.

use serde_json::{Map, Value};
use yulius_domain::task::GoalOutcome;
use yulius_domain::usage::TokenUsage;

use crate::json::{bool_of, obj_of, owned_str, parse_object, shorten, str_of, u64_of};
use crate::observe::Observed;
use crate::time::{from_epoch_seconds, parse_rfc3339};

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

/// The user lines Claude Code 2.1.292 writes when the person interrupts a turn (Esc, or Ctrl+C
/// while the agent works): a text block with exactly one of these.
const INTERRUPT_MARKERS: [&str; 2] = [
    "[Request interrupted by user]",
    "[Request interrupted by user for tool use]",
];

/// `apiError` of the error record Claude Code writes when the account's usage limit rejects a
/// request (alongside `error: "rate_limit"` and HTTP 429).
const USAGE_LIMIT_API_ERROR: &str = "usage_limit_reached";

/// Lower-case marks of a too-long prompt in an error record: Claude Code's own text ("Prompt is
/// too long", its `PROMPT_TOO_LONG` constant), the API's error (`errorDetails`) or its code.
const PROMPT_TOO_LONG_MARKS: [&str; 2] = ["prompt is too long", "prompt_too_long"];

/// What Claude Code reports when a compaction fails.
const COMPACT_ERROR_TEXT: &str = "Error during compaction";

/// The error output of a local command (`/compact`) in a transcript record.
const LOCAL_COMMAND_STDERR: &str = "<local-command-stderr>";

/// Normalizes one Claude Code transcript line.
#[must_use]
pub fn observe_line(line: &str) -> Vec<Observed> {
    let Some(record) = parse_object(line) else {
        return vec![Observed::Malformed];
    };
    match str_of(&record, "type") {
        Some("user") => observe_user(&record),
        Some("assistant") => observe_assistant(&record),
        Some("attachment") => observe_attachment(&record),
        Some("system") => observe_system(&record),
        Some(_) => Vec::new(),
        None => vec![Observed::Malformed],
    }
}

/// The session goal's state from a `goal_status` attachment (see the module docs).
fn observe_attachment(record: &Map<String, Value>) -> Vec<Observed> {
    let Some(attachment) = obj_of(record, "attachment") else {
        return Vec::new();
    };
    if bool_of(record, "isSidechain") || str_of(attachment, "type") != Some("goal_status") {
        return Vec::new();
    }
    let met = bool_of(attachment, "met");
    let outcome = if bool_of(attachment, "sentinel") {
        if met {
            GoalOutcome::Cleared
        } else {
            GoalOutcome::Set
        }
    } else if met {
        GoalOutcome::Met
    } else if bool_of(attachment, "failed") {
        GoalOutcome::Failed
    } else {
        GoalOutcome::NotMet
    };
    vec![Observed::Goal { outcome }]
}

/// The record Claude Code writes when a turn closes. `stop_hook_summary` is not one: it is also
/// written when a Stop hook blocks (a goal not met, a notice) and the turn goes on.
fn observe_system(record: &Map<String, Value>) -> Vec<Observed> {
    if bool_of(record, "isSidechain") {
        return Vec::new();
    }
    match str_of(record, "subtype") {
        Some("turn_duration") => vec![Observed::TurnEnded {
            background_agents: u64_of(record, "pendingBackgroundAgentCount")
                .and_then(|n| u32::try_from(n).ok()),
        }],
        Some("compact_boundary") => vec![Observed::Compacted],
        _ if str_of(record, "content").is_some_and(|c| c.contains(COMPACT_ERROR_TEXT)) => {
            vec![Observed::CompactFailed]
        }
        _ => Vec::new(),
    }
}

/// A user record that carries a failed compaction's error output, not a prompt.
fn compact_failed(message: &Map<String, Value>) -> bool {
    let text = message_text(message);
    text.trim_start().starts_with(LOCAL_COMMAND_STDERR) && text.contains(COMPACT_ERROR_TEXT)
}

/// An error record that says the prompt is too long for the model's context (#213).
/// Only error records are asked, so an agent's answer that quotes the words is not one.
fn prompt_too_long(record: &Map<String, Value>, text: &str) -> bool {
    let says = |value: &str| {
        let lower = value.to_lowercase();
        PROMPT_TOO_LONG_MARKS
            .iter()
            .any(|mark| lower.contains(mark))
    };
    says(text)
        || ["errorDetails", "apiError"]
            .iter()
            .any(|key| str_of(record, key).is_some_and(says))
}

fn observe_user(record: &Map<String, Value>) -> Vec<Observed> {
    let Some(message) = obj_of(record, "message") else {
        return vec![Observed::Malformed];
    };
    if bool_of(record, "isSidechain") || bool_of(record, "isMeta") || !is_prompt(message) {
        return Vec::new();
    }
    if compact_failed(message) {
        return vec![Observed::CompactFailed];
    }
    let at = str_of(record, "timestamp").and_then(parse_rfc3339);
    let turn = if is_interrupt(message) {
        Observed::TurnInterrupted { at }
    } else {
        Observed::UserTurn { at }
    };
    let Some(session_id) = owned_str(record, "sessionId") else {
        return vec![turn];
    };
    vec![
        Observed::SessionMeta {
            session_id,
            cwd: owned_str(record, "cwd"),
            model: None,
            version: owned_str(record, "version"),
        },
        turn,
    ]
}

/// A user line that only says the person interrupted the turn ([`INTERRUPT_MARKERS`]).
fn is_interrupt(message: &Map<String, Value>) -> bool {
    let marker = |text: &str| INTERRUPT_MARKERS.contains(&text.trim());
    match message.get("content") {
        Some(Value::String(text)) => marker(text),
        Some(Value::Array(blocks)) => {
            let mut texts = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str));
            texts.next().is_some_and(marker) && texts.next().is_none()
        }
        _ => false,
    }
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
        if let Some(limit) = structured_limit(record, &text) {
            return vec![limit];
        }
        if prompt_too_long(record, &text) {
            // A sub-agent's full context is its own; the session goes on.
            return if bool_of(record, "isSidechain") {
                Vec::new()
            } else {
                vec![Observed::ContextFull]
            };
        }
        return limit_signal(&text).into_iter().collect();
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
        out.push(Observed::TurnComplete {
            duration_ms: None,
            at: (!bool_of(record, "isSidechain"))
                .then(|| str_of(record, "timestamp").and_then(parse_rfc3339))
                .flatten(),
        });
    } else if !bool_of(record, "isSidechain")
        && owned_str(message, "model").as_deref() != Some(SYNTHETIC_MODEL)
    {
        // A sub-agent's lines (sidechain) and the lines Claude Code writes itself ("No response
        // requested.", `<synthetic>`, `stop_sequence`) are not the session's work.
        out.push(Observed::AgentWork {
            at: str_of(record, "timestamp").and_then(parse_rfc3339),
        });
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

    /// Deterministic xorshift generator for randomized tests (no extra dependency).
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        fn below(&mut self, bound: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(bound.max(1)).unwrap_or(1)).unwrap_or(0)
        }

        /// Text made of `pieces`, random bytes (lossy UTF-8) and multi-byte characters.
        fn text(&mut self, pieces: &[&str], max_parts: usize) -> String {
            let mut out = String::new();
            for _ in 0..self.below(max_parts + 1) {
                match self.below(10) {
                    0 => {
                        let bytes: Vec<u8> = (0..self.below(8))
                            .map(|_| (self.next() & 0xff) as u8)
                            .collect();
                        out.push_str(&String::from_utf8_lossy(&bytes));
                    }
                    1 => out
                        .push_str(["é", "😀", "\u{0}", "\r", "\t", "ﬀ", "\u{200b}"][self.below(7)]),
                    _ => out.push_str(pieces[self.below(pieces.len())]),
                }
            }
            out
        }
    }

    /// Randomized transcript lines (#76): the observer never panics on malformed records.
    #[test]
    fn random_transcript_lines_never_panic() {
        let pieces = [
            "{",
            "}",
            "[",
            "]",
            ",",
            ":",
            "\"type\"",
            "\"assistant\"",
            "\"user\"",
            "\"message\"",
            "\"usage\"",
            "\"input_tokens\"",
            "\"output_tokens\"",
            "\"stop_reason\"",
            "\"end_turn\"",
            "\"isApiErrorMessage\"",
            "true",
            "false",
            "null",
            "\"apiError\"",
            "\"usage_limit_reached\"",
            "\"rate_limit_info\"",
            "\"resetsAt\"",
            "1760000000",
            "-1",
            "1e400",
            "\"rateLimitType\"",
            "\"five_hour\"",
            "\"content\"",
            "\"text\"",
            "\"session limit · resets 3pm\"",
            "\"timestamp\"",
            "\"2026-10-09T10:00:00Z\"",
            "\"attachment\"",
            "\"goal_status\"",
            "\"met\"",
            "\"sentinel\"",
            "\"failed\"",
            "\"system\"",
            "\"subtype\"",
            "\"turn_duration\"",
            "\"stop_hook_summary\"",
        ];
        let mut rng = Rng(0x0dd5_eed5_cafe_f00d);
        for _ in 0..6_000 {
            let line = rng.text(&pieces, 30);
            let _ = observe_line(&line);
        }
    }

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

    /// TSK-014: `goal_status` attachments map to the goal's outcome whatever their wording;
    /// other attachments, sidechain goals and attachments without a body are ignored.
    #[test]
    fn goal_status_attachments() {
        let goal = |body: &str| {
            observe_line(&format!(
                r#"{{"type":"attachment","isSidechain":false,"attachment":{body}}}"#
            ))
        };
        let outcome = |o| vec![Observed::Goal { outcome: o }];
        assert_eq!(
            goal(r#"{"type":"goal_status","met":false,"sentinel":true,"condition":"c"}"#),
            outcome(GoalOutcome::Set)
        );
        assert_eq!(
            goal(r#"{"type":"goal_status","met":true,"sentinel":true,"condition":"c"}"#),
            outcome(GoalOutcome::Cleared)
        );
        assert_eq!(
            goal(r#"{"type":"goal_status","met":false,"condition":"c","reason":"r"}"#),
            outcome(GoalOutcome::NotMet)
        );
        assert_eq!(
            goal(r#"{"type":"goal_status","met":true,"condition":"c","iterations":1}"#),
            outcome(GoalOutcome::Met)
        );
        assert_eq!(
            goal(r#"{"type":"goal_status","met":false,"failed":true,"condition":"c"}"#),
            outcome(GoalOutcome::Failed)
        );
        assert!(goal(r#"{"type":"todo_reminder"}"#).is_empty());
        assert!(goal(r#""goal_status""#).is_empty());
        assert!(observe_line(r#"{"type":"attachment"}"#).is_empty());
        assert!(
            observe_line(
                r#"{"type":"attachment","isSidechain":true,"attachment":{"type":"goal_status","met":true}}"#
            )
            .is_empty()
        );
        assert_eq!(
            observe_line(r#"{"type":"system","subtype":"turn_duration","durationMs":5}"#),
            vec![Observed::TurnEnded {
                background_agents: None
            }]
        );
        assert!(observe_line(r#"{"type":"system","subtype":"away_summary"}"#).is_empty());
        assert_eq!(
            observe_line(
                r#"{"type":"user","timestamp":"2026-10-10T05:38:01.385Z","message":{"content":"go"}}"#
            ),
            vec![Observed::UserTurn {
                at: Some(Timestamp(1_791_610_681_385))
            }]
        );
    }

    /// STA-001, STA-005 (SPEC-032): only `turn_duration` is a turn's true end and it carries the
    /// background agents Claude Code still waits for; `stop_hook_summary` is also written when a
    /// Stop hook blocks and the turn goes on, so it is no observation. Lines in the shapes of the
    /// transcripts of tasks #23 and #24 (Claude Code 2.1.292).
    #[test]
    fn a_turn_end_says_how_many_background_agents_are_pending() {
        let at = Some(Timestamp(1_791_610_681_385));
        let end = |extra: &str| {
            observe_line(&format!(
                r#"{{"type":"system","subtype":"turn_duration","durationMs":94387,"messageCount":953{extra}}}"#
            ))
        };
        assert_eq!(
            end(r#","pendingBackgroundAgentCount":1"#),
            vec![Observed::TurnEnded {
                background_agents: Some(1)
            }]
        );
        assert_eq!(
            end(r#","pendingBackgroundAgentCount":0"#),
            vec![Observed::TurnEnded {
                background_agents: Some(0)
            }]
        );
        assert_eq!(
            end(""),
            vec![Observed::TurnEnded {
                background_agents: None
            }],
            "an older CLI does not say"
        );
        assert_eq!(
            end(r#","pendingBackgroundAgentCount":"x""#),
            vec![Observed::TurnEnded {
                background_agents: None
            }]
        );
        assert!(
            observe_line(
                r#"{"type":"system","subtype":"stop_hook_summary","hookCount":1,"preventedContinuation":false,"hasOutput":true}"#
            )
            .is_empty(),
            "a blocked stop is not the end of the turn"
        );
        // An assistant line that is not the turn's end shows the agent works; its end does not.
        let assistant = |stop: &str, sidechain: bool| {
            observe_line(&format!(
                r#"{{"type":"assistant","isSidechain":{sidechain},"timestamp":"2026-10-10T05:38:01.385Z","message":{{"id":"m1","model":"claude-opus-5-5","role":"assistant","content":[],"stop_reason":{stop}}}}}"#
            ))
        };
        let work = |lines: Vec<Observed>| lines.contains(&Observed::AgentWork { at });
        assert!(work(assistant(r#""tool_use""#, false)));
        assert!(work(assistant("null", false)));
        assert!(!work(assistant(r#""end_turn""#, false)));
        // Claude Code's own synthetic line ("No response requested.") is not work.
        let synthetic = observe_line(
            r#"{"type":"assistant","isSidechain":false,"timestamp":"2026-10-10T05:38:01.385Z","message":{"id":"m2","model":"<synthetic>","role":"assistant","content":[{"type":"text","text":"No response requested."}],"stop_reason":"stop_sequence"}}"#,
        );
        assert!(
            !synthetic
                .iter()
                .any(|o| matches!(o, Observed::AgentWork { .. }))
        );
        assert!(
            assistant(r#""end_turn""#, false)
                .iter()
                .any(|o| matches!(o, Observed::TurnComplete { at: Some(_), .. }))
        );
        // A sub-agent's lines are not the session's turn.
        assert!(!work(assistant(r#""tool_use""#, true)));
    }

    /// Esc or Ctrl+C during a turn (Claude Code 2.1.292, shape seen on the deployed instance):
    /// the turn ends, it is not a new prompt.
    #[test]
    fn an_interrupt_line_ends_the_turn() {
        let at = Some(Timestamp(1_791_610_681_385));
        for text in INTERRUPT_MARKERS {
            let line = json_line(text);
            assert_eq!(
                observe_line(&line),
                vec![
                    Observed::SessionMeta {
                        session_id: "s1".into(),
                        cwd: None,
                        model: None,
                        version: None,
                    },
                    Observed::TurnInterrupted { at }
                ],
                "{text}"
            );
        }
        // A prompt that mentions an interruption is still a prompt.
        assert_eq!(
            observe_line(&json_line(
                "why did you stop? [Request interrupted by user]"
            ))
            .last(),
            Some(&Observed::UserTurn { at })
        );
    }

    fn json_line(text: &str) -> String {
        serde_json::json!({
            "type": "user",
            "sessionId": "s1",
            "timestamp": "2026-10-10T05:38:01.385Z",
            "userType": "external",
            "isSidechain": false,
            "message": {"role": "user", "content": [{"type": "text", "text": text}]}
        })
        .to_string()
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

    /// The record Claude Code 2.1.292 writes when the API rejects a prompt as too long (#213):
    /// its API-error helper with `error: "invalid_request"` and the `PROMPT_TOO_LONG` text.
    #[test]
    fn a_prompt_too_long_error_is_a_full_context() {
        let line = r#"{"type":"assistant","isSidechain":false,"isApiErrorMessage":true,"error":"invalid_request","errorDetails":"prompt is too long: 213000 tokens > 200000 maximum","message":{"model":"<synthetic>","id":"m","type":"message","role":"assistant","content":[{"type":"text","text":"Prompt is too long"}],"stop_reason":"stop_sequence"}}"#;
        assert_eq!(observe_line(line), vec![Observed::ContextFull]);
        // Only the API's own wording (another CLI build's text), or its error code.
        let details = r#"{"type":"assistant","isApiErrorMessage":true,"error":"invalid_request","errorDetails":"400 {\"error\":{\"message\":\"Prompt is too long: 201000 tokens\"}}","message":{"model":"<synthetic>","content":[{"type":"text","text":"API Error"}]}}"#;
        assert_eq!(observe_line(details), vec![Observed::ContextFull]);
        let code = r#"{"type":"assistant","isApiErrorMessage":true,"apiError":"prompt_too_long","message":{"model":"<synthetic>","content":[]}}"#;
        assert_eq!(observe_line(code), vec![Observed::ContextFull]);
        // Only the error text says so (an older or newer CLI's wording).
        let text = r#"{"type":"assistant","isApiErrorMessage":true,"message":{"model":"<synthetic>","content":[{"type":"text","text":"API Error: 400 {\"error\":{\"message\":\"prompt is too long: 201000 tokens > 200000 maximum\"}}"}]}}"#;
        assert_eq!(observe_line(text), vec![Observed::ContextFull]);
        // A sub-agent's full context is its own: the session goes on.
        let sidechain = line.replace(r#""isSidechain":false"#, r#""isSidechain":true"#);
        assert_eq!(observe_line(&sidechain), Vec::<Observed>::new());
        // An agent's ordinary answer that quotes the words is no error record.
        let answer = r#"{"type":"assistant","message":{"model":"claude","id":"m2","content":[{"type":"text","text":"Prompt is too long"}],"stop_reason":"end_turn"}}"#;
        assert!(!observe_line(answer).contains(&Observed::ContextFull));
        // Another API error is still nothing (or a limit).
        let other = r#"{"type":"assistant","isApiErrorMessage":true,"error":"invalid_request","message":{"model":"<synthetic>","content":[{"type":"text","text":"Credit balance is too low"}]}}"#;
        assert_eq!(observe_line(other), Vec::<Observed>::new());
    }

    /// A compaction's boundary record and the error a failed `/compact` reports.
    #[test]
    fn compactions_and_their_failures_are_observed() {
        let boundary = r#"{"type":"system","subtype":"compact_boundary","content":"Conversation compacted","isMeta":false,"isSidechain":false,"level":"info","compactMetadata":{"trigger":"manual","preTokens":190000}}"#;
        assert_eq!(observe_line(boundary), vec![Observed::Compacted]);
        let sidechain = boundary.replace(r#""isSidechain":false"#, r#""isSidechain":true"#);
        assert_eq!(observe_line(&sidechain), Vec::<Observed>::new());
        let system = r#"{"type":"system","subtype":"local_command","content":"<local-command-stderr>Error: Error during compaction: Conversation too long. Press esc twice to go up a few messages and try again.</local-command-stderr>","isSidechain":false}"#;
        assert_eq!(observe_line(system), vec![Observed::CompactFailed]);
        let user = json_line(
            "<local-command-stderr>Error: Error during compaction: Error: API Error</local-command-stderr>",
        );
        assert_eq!(observe_line(&user), vec![Observed::CompactFailed]);
        // A person's prompt that quotes the error output is a prompt.
        let quoted = json_line(
            "why did this fail? <local-command-stderr>Error: Error during compaction</local-command-stderr>",
        );
        assert!(!observe_line(&quoted).contains(&Observed::CompactFailed));
        // A person's prompt about compaction is a prompt.
        let prompt = json_line("why did the Error during compaction happen?");
        let observed = observe_line(&prompt);
        assert!(
            observed
                .iter()
                .any(|o| matches!(o, Observed::UserTurn { .. }))
        );
        assert!(!observed.contains(&Observed::CompactFailed));
    }
}
