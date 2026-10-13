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
use yulius_domain::time::Timestamp;
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

/// How the user records begin that Claude Code writes for itself, not as a prompt the model
/// answers (SPEC-085 STT-001): a slash command's echo, a local command's caveat and output, and
/// the shell mode's input and output (`!cmd`). A command that runs the model (`/goal`) is followed
/// by the model's own work, which shows the turn.
const OWN_RECORD_TAGS: [&str; 8] = [
    "<command-name>",
    "<command-message>",
    "<local-command-caveat>",
    "<local-command-stdout>",
    LOCAL_COMMAND_STDERR,
    "<bash-input>",
    "<bash-stdout>",
    "<bash-stderr>",
];

/// The longest a Monitor stream lives (Claude Code caps `timeoutMs` at 30 minutes; this bound
/// leaves room), and how long one lives when its record does not say.
const MONITOR_MAX_MS: u64 = 60 * 60 * 1000;
const MONITOR_DEFAULT_MS: u64 = 5 * 60 * 1000;

/// How the tool result of a Monitor start begins ("Monitor started (task b1, expires in 15m …").
const MONITOR_STARTED: &str = "Monitor started (task ";

/// The longest task id taken from a notification or a tool result.
const MAX_TASK_ID: usize = 128;

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

/// When a transcript line was recorded (its top-level `timestamp`), if it says. Records Claude
/// Code writes about the session itself (`last-prompt`, `mode`, …) carry no time.
#[must_use]
pub fn record_time(line: &str) -> Option<Timestamp> {
    parse_object(line)
        .as_ref()
        .and_then(|record| str_of(record, "timestamp"))
        .and_then(parse_rfc3339)
}

/// The session goal's state from a `goal_status` attachment (see the module docs).
fn observe_attachment(record: &Map<String, Value>) -> Vec<Observed> {
    let Some(attachment) = obj_of(record, "attachment") else {
        return Vec::new();
    };
    if bool_of(record, "isSidechain") {
        return Vec::new();
    }
    // A notification delivered while the agent works (SPEC-079 BGW-002).
    if str_of(attachment, "type") == Some("queued_command") {
        return str_of(attachment, "prompt").map_or_else(Vec::new, background_ended);
    }
    if str_of(attachment, "type") != Some("goal_status") {
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
            at: str_of(record, "timestamp").and_then(parse_rfc3339),
            duration_ms: u64_of(record, "durationMs"),
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
    if bool_of(record, "isSidechain") || bool_of(record, "isMeta") {
        return Vec::new();
    }
    if !is_prompt(message) {
        return background_started(record, message).into_iter().collect();
    }
    if compact_failed(message) {
        return vec![Observed::CompactFailed];
    }
    let at = str_of(record, "timestamp").and_then(parse_rfc3339);
    let text = message_text(message);
    let session = owned_str(record, "sessionId").map(|session_id| Observed::SessionMeta {
        session_id,
        cwd: owned_str(record, "cwd"),
        model: None,
        version: owned_str(record, "version"),
    });
    // A record Claude Code writes for itself starts no turn: an idle compaction's summary made a
    // finished task read Working for good (SPEC-085 STT-001).
    if own_record(record, &text) {
        return session.into_iter().collect();
    }
    let turn = if is_interrupt(message) {
        Observed::TurnInterrupted { at }
    } else {
        Observed::UserTurn {
            at,
            wake: text.trim_start().starts_with("<task-notification>"),
        }
    };
    // A wake-up prompt says which background work ended.
    let mut out = background_ended(&text);
    out.extend(session);
    out.push(turn);
    out
}

/// A user record with text that Claude Code wrote for itself, not a prompt for the model
/// (SPEC-085 STT-001): the summary a compaction continues from (`isCompactSummary`, shown in the
/// transcript only), a command's echo and output ([`OWN_RECORD_TAGS`]), or the compaction command
/// as typed (`/compact`, written before the compaction runs; no turn follows it, STT-005).
fn own_record(record: &Map<String, Value>, text: &str) -> bool {
    let text = text.trim_start();
    bool_of(record, "isCompactSummary")
        || bool_of(record, "isVisibleInTranscriptOnly")
        || OWN_RECORD_TAGS.iter().any(|tag| text.starts_with(tag))
        || yulius_domain::harness::is_compact_command(text)
}

/// A tool result that started background work the session is woken by (SPEC-079 BGW-002): a
/// Monitor stream (its text "Monitor started (task …", `toolUseResult` `{taskId, timeoutMs}`) or
/// a background agent (`{status: "async_launched", agentId}`).
fn background_started(
    record: &Map<String, Value>,
    message: &Map<String, Value>,
) -> Option<Observed> {
    let result = obj_of(record, "toolUseResult")?;
    let task_id =
        |key| owned_str(result, key).filter(|id| !id.is_empty() && id.len() <= MAX_TASK_ID);
    let (id, agent, expires_in_ms) = if str_of(result, "status") == Some("async_launched") {
        (task_id("agentId")?, true, None)
    } else if tool_result_text(message).contains(MONITOR_STARTED) {
        let ms = u64_of(result, "timeoutMs").unwrap_or(MONITOR_DEFAULT_MS);
        (task_id("taskId")?, false, Some(ms.min(MONITOR_MAX_MS)))
    } else {
        return None;
    };
    Some(Observed::BackgroundStarted {
        id,
        agent,
        expires_in_ms,
        at: str_of(record, "timestamp").and_then(parse_rfc3339),
    })
}

/// The text of a user line's tool results (string content or their text blocks).
fn tool_result_text(message: &Map<String, Value>) -> String {
    let Some(Value::Array(blocks)) = message.get("content") else {
        return String::new();
    };
    let mut out = String::new();
    for block in blocks
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
    {
        match block.get("content") {
            Some(Value::String(text)) => out.push_str(text),
            Some(Value::Array(parts)) => {
                for text in parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str))
                {
                    out.push_str(text);
                }
            }
            _ => {}
        }
    }
    out
}

/// The background work the `<task-notification>` blocks of `text` say ended: those with a
/// `<status>` (completed, failed, stopped) and a Monitor's expiry event. A Monitor's other events
/// leave it running.
fn background_ended(text: &str) -> Vec<Observed> {
    const OPEN: &str = "<task-notification>";
    const CLOSE: &str = "</task-notification>";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(OPEN) {
        let after = &rest[start + OPEN.len()..];
        let (block, next) = after.split_once(CLOSE).unwrap_or((after, ""));
        rest = next;
        let id = block
            .split_once("<task-id>")
            .and_then(|(_, tail)| tail.split_once("</task-id>"))
            .map(|(id, _)| id.trim())
            .filter(|id| !id.is_empty() && id.len() <= MAX_TASK_ID);
        if let Some(id) = id
            && (block.contains("<status>") || block.contains("[Monitor expired"))
        {
            out.push(Observed::BackgroundEnded { id: id.to_owned() });
        }
    }
    out
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
        let at = str_of(record, "timestamp").and_then(parse_rfc3339);
        if let Some(limit) = structured_limit(record, &text) {
            return vec![recorded_at(limit, at)];
        }
        if prompt_too_long(record, &text) {
            // A sub-agent's full context is its own; the session goes on.
            return if bool_of(record, "isSidechain") {
                Vec::new()
            } else {
                vec![Observed::ContextFull]
            };
        }
        return limit_signal(&text)
            .map(|limit| recorded_at(limit, at))
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

/// A limit observation with the time its record was written.
fn recorded_at(limit: Observed, when: Option<Timestamp>) -> Observed {
    match limit {
        Observed::LimitReached {
            resets_at,
            message,
            window_minutes,
            ..
        } => Observed::LimitReached {
            resets_at,
            message,
            window_minutes,
            at: when,
        },
        other => other,
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
        at: None,
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
        at: None,
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
                at: None,
            })
        );
        assert_eq!(
            limit_signal("5-hour limit reached ∙ resets 3pm"),
            Some(Observed::LimitReached {
                resets_at: None,
                message: "5-hour limit reached ∙ resets 3pm".into(),
                window_minutes: Some(300),
                at: None,
            })
        );
        assert_eq!(limit_signal("API Error: 500 internal server error"), None);
        assert_eq!(
            limit_signal("You've hit your session limit · resets 10:20am (Europe/Berlin)"),
            Some(Observed::LimitReached {
                resets_at: None,
                message: "You've hit your session limit · resets 10:20am (Europe/Berlin)".into(),
                window_minutes: Some(300),
                at: None,
            })
        );
        assert_eq!(
            limit_signal("You've hit your weekly limit · resets Oct 12"),
            Some(Observed::LimitReached {
                resets_at: None,
                message: "You've hit your weekly limit · resets Oct 12".into(),
                window_minutes: Some(10080),
                at: None,
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
            observe_line(
                r#"{"type":"system","subtype":"turn_duration","durationMs":5,"timestamp":"2026-10-10T05:38:01.385Z"}"#
            ),
            vec![Observed::TurnEnded {
                background_agents: None,
                at: Some(Timestamp(1_791_610_681_385)),
                duration_ms: Some(5),
            }]
        );
        assert!(observe_line(r#"{"type":"system","subtype":"away_summary"}"#).is_empty());
        assert_eq!(
            observe_line(
                r#"{"type":"user","timestamp":"2026-10-10T05:38:01.385Z","message":{"content":"go"}}"#
            ),
            vec![Observed::UserTurn {
                at: Some(Timestamp(1_791_610_681_385)),
                wake: false
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
                background_agents: Some(1),
                at: None,
                duration_ms: Some(94_387),
            }]
        );
        assert_eq!(
            end(r#","pendingBackgroundAgentCount":0"#),
            vec![Observed::TurnEnded {
                background_agents: Some(0),
                at: None,
                duration_ms: Some(94_387),
            }]
        );
        assert_eq!(
            end(""),
            vec![Observed::TurnEnded {
                background_agents: None,
                at: None,
                duration_ms: Some(94_387),
            }],
            "an older CLI does not say"
        );
        assert_eq!(
            end(r#","pendingBackgroundAgentCount":"x""#),
            vec![Observed::TurnEnded {
                background_agents: None,
                at: None,
                duration_ms: Some(94_387),
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

    /// SPEC-079 BGW-002: a Monitor's start and a background agent's launch (their tool results'
    /// `toolUseResult`), and the task notifications that end them, as prompt or queued command.
    /// Shapes of the sandbox transcripts (Claude Code 2.1.292).
    #[test]
    fn background_launches_and_their_ends_are_observed() {
        let at = Some(Timestamp(1_791_610_681_385));
        let result = |tur: &str, sidechain: bool| {
            observe_line(&format!(
                r#"{{"type":"user","isSidechain":{sidechain},"timestamp":"2026-10-10T05:38:01.385Z","message":{{"role":"user","content":[{{"tool_use_id":"toolu_1","type":"tool_result","content":"Monitor started (task bppf4z917, expires in 15m unless the source ends first)"}}]}},"toolUseResult":{tur}}}"#
            ))
        };
        assert_eq!(
            result(
                r#"{"taskId":"bppf4z917","timeoutMs":900000,"persistent":false}"#,
                false
            ),
            vec![Observed::BackgroundStarted {
                id: "bppf4z917".into(),
                agent: false,
                expires_in_ms: Some(900_000),
                at
            }]
        );
        assert_eq!(
            result(r#"{"taskId":"b1","timeoutMs":99999999}"#, false),
            vec![Observed::BackgroundStarted {
                id: "b1".into(),
                agent: false,
                expires_in_ms: Some(MONITOR_MAX_MS),
                at
            }],
            "capped"
        );
        assert_eq!(
            result(
                r#"{"isAsync":true,"status":"async_launched","agentId":"a4ed8aab8c9658147","description":"Review"}"#,
                false
            ),
            vec![Observed::BackgroundStarted {
                id: "a4ed8aab8c9658147".into(),
                agent: true,
                expires_in_ms: None,
                at
            }]
        );
        // No `timeoutMs`: Claude Code's default of 5 minutes.
        assert_eq!(
            result(r#"{"taskId":"b3"}"#, false),
            vec![Observed::BackgroundStarted {
                id: "b3".into(),
                agent: false,
                expires_in_ms: Some(MONITOR_DEFAULT_MS),
                at
            }]
        );
        // Another tool's result of the same shape is no Monitor.
        let other = observe_line(
            r#"{"type":"user","isSidechain":false,"message":{"role":"user","content":[{"tool_use_id":"t","type":"tool_result","content":"Scheduled."}]},"toolUseResult":{"taskId":"b4","timeoutMs":1000}}"#,
        );
        assert!(other.is_empty());
        // A foreground agent's result, a plain tool result and a sub-agent's launch are none.
        assert!(result(r#"{"status":"completed","agentId":"a1"}"#, false).is_empty());
        assert!(result(r#"{"stdout":"","stderr":""}"#, false).is_empty());
        assert!(result(r#"{"taskId":"b2","timeoutMs":1000}"#, true).is_empty());

        let ended = |id: &str| Observed::BackgroundEnded { id: id.into() };
        let notification =
            |body: &str| format!("<task-notification>\n<task-id>{body}</task-notification>");
        let end = notification(
            "bppf4z917</task-id>\n<status>completed</status>\n<summary>Monitor \"cargo check\" stream ended</summary>\n",
        );
        let event = notification(
            "bx</task-id>\n<summary>Monitor event: \"ci\"</summary>\n<event>step 1 ok</event>\n",
        );
        let expired = notification(
            "b71u1sesi</task-id>\n<summary>Monitor event: \"wait\"</summary>\n<event>[Monitor expired after 10m with no events delivered.]</event>\n",
        );
        // A wake-up prompt: the end, then the prompt's turn.
        let prompt = observe_line(
            &serde_json::json!({"type": "user", "isSidechain": false,
                "timestamp": "2026-10-10T05:38:01.385Z",
                "message": {"role": "user", "content": format!("{end}\n{event}\n{expired}")}})
            .to_string(),
        );
        assert_eq!(
            prompt,
            vec![
                ended("bppf4z917"),
                ended("b71u1sesi"),
                Observed::UserTurn { at, wake: true }
            ]
        );
        // Delivered while the agent works.
        let queued = |prompt: &str, sidechain: bool| {
            observe_line(
                &serde_json::json!({"type": "attachment", "isSidechain": sidechain,
                    "attachment": {"type": "queued_command", "commandMode": "task-notification",
                    "prompt": prompt}})
                .to_string(),
            )
        };
        let stopped = notification("a1</task-id>\n<status>stopped</status>\n");
        assert_eq!(queued(&stopped, false), vec![ended("a1")]);
        assert!(queued(&event, false).is_empty());
        assert!(queued(&stopped, true).is_empty());
        assert!(
            queued(
                "<task-notification><task-id></task-id><status>completed</status>",
                false
            )
            .is_empty()
        );
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
            Some(&Observed::UserTurn { at, wake: false })
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
                at: None,
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
                at: None,
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

    /// SPEC-085 STT-001: the user records Claude Code 2.1.292 writes for itself start no turn.
    /// Shapes from the sandbox transcripts of 2026-10-12/13 (texts shortened, ids made up): task
    /// #60's idle compaction ("Compacted while idle, before the prompt cache expired", 19:42 UTC,
    /// an hour after its goal was achieved), whose summary record made the Done task read Working
    /// until it was stopped; and the records of `/goal` and `/compact`. Each still names the
    /// session; a typed prompt and a wake-up still start a turn.
    #[test]
    fn records_claude_code_writes_for_itself_start_no_turn() {
        let at = Some(Timestamp(1_791_610_681_385));
        let session = Observed::SessionMeta {
            session_id: "f77c8148".into(),
            cwd: Some("/w".into()),
            model: None,
            version: Some("2.1.292".into()),
        };
        let user = |content: serde_json::Value, extra: serde_json::Value| {
            let mut record = serde_json::json!({"parentUuid": "p", "isSidechain": false,
                "promptId": "a4e3a8bf", "type": "user",
                "message": {"role": "user", "content": content}, "uuid": "u",
                "timestamp": "2026-10-10T05:38:01.385Z", "userType": "external",
                "entrypoint": "cli", "cwd": "/w", "sessionId": "f77c8148", "version": "2.1.292",
                "gitBranch": "feat/60-x"});
            if let (Some(record), Some(extra)) = (record.as_object_mut(), extra.as_object()) {
                record.extend(extra.clone());
            }
            observe_line(&record.to_string())
        };
        let none = serde_json::json!({});
        // The idle compaction: its boundary, the summary it continues from, the files it reads
        // again, its SessionStart hook and its notice.
        let boundary = r#"{"parentUuid":null,"logicalParentUuid":"l","isSidechain":false,"type":"system","subtype":"compact_boundary","content":"Conversation compacted","isMeta":false,"timestamp":"2026-10-13T19:42:51.383Z","uuid":"b","level":"info","compactMetadata":{"trigger":"auto","preTokens":400160,"durationMs":47527,"postTokens":8602},"sessionId":"f77c8148","version":"2.1.292"}"#;
        assert_eq!(observe_line(boundary), vec![Observed::Compacted]);
        let summary = user(
            serde_json::json!(
                "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation."
            ),
            serde_json::json!({"isVisibleInTranscriptOnly": true, "isCompactSummary": true,
                "turnPosition": {"promptIndex": 1, "turnIndex": 14}}),
        );
        assert_eq!(summary, vec![session.clone()], "the summary is no prompt");
        for attachment in [
            r#"{"isSidechain":false,"type":"attachment","timestamp":"2026-10-13T19:42:51.681Z","attachment":{"type":"file","filename":"/tmp/x/tasks/b.output","content":{"type":"text","file":{"filePath":"/tmp/x/tasks/b.output","content":"Waiting: checks running","numLines":6,"startLine":1,"totalLines":6}}}}"#,
            r#"{"isSidechain":false,"type":"attachment","timestamp":"2026-10-13T19:42:51.681Z","attachment":{"type":"compact_file_reference","filename":"/w/crates/yulius-app/src/finish.rs","displayPath":"crates/yulius-app/src/finish.rs"}}"#,
            r#"{"isSidechain":false,"type":"attachment","timestamp":"2026-10-13T19:42:51.794Z","attachment":{"type":"hook_success","hookName":"SessionStart:compact","hookEvent":"SessionStart","content":"","exitCode":0}}"#,
            r#"{"isSidechain":false,"type":"system","subtype":"informational","content":"Compacted while idle, before the prompt cache expired","isMeta":false,"timestamp":"2026-10-13T19:42:51.851Z","level":"notice"}"#,
        ] {
            assert_eq!(
                observe_line(attachment),
                Vec::<Observed>::new(),
                "{attachment}"
            );
        }
        // `/goal` and `/compact`: the command's echo and its output. The model's own work after
        // `/goal` shows its turn.
        let echo = user(
            serde_json::json!(
                "<command-name>/goal</command-name>\n            <command-message>goal</command-message>\n            <command-args>Everything requested is done.</command-args>"
            ),
            none.clone(),
        );
        assert_eq!(echo, vec![session.clone()]);
        let goal_set = user(
            serde_json::json!(
                "<local-command-stdout>Goal set: Everything requested is done.</local-command-stdout>"
            ),
            serde_json::json!({"turnOrigin": "human"}),
        );
        assert_eq!(goal_set, vec![session.clone()]);
        let compacted = user(
            serde_json::json!(
                "<local-command-stdout>\u{1b}[2mCompacted (ctrl+o to see full summary)\u{1b}[22m</local-command-stdout>"
            ),
            none.clone(),
        );
        assert_eq!(compacted, vec![session.clone()]);
        // STT-005: `/compact` as typed, the line Claude Code writes before it compacts (task #8,
        // 2026-10-12 04:22:29; no `origin`, unlike a typed prompt). No turn follows it.
        for typed in ["/compact", "/compact keep the plan and the failing test"] {
            assert_eq!(
                user(serde_json::json!(typed), none.clone()),
                vec![session.clone()],
                "{typed}"
            );
        }
        let shell = user(
            serde_json::json!([{"type": "text", "text": "<bash-input>git status</bash-input>"}]),
            none.clone(),
        );
        assert_eq!(shell, vec![session.clone()]);
        // A typed prompt, and Claude Code's wake-up, start a turn.
        let typed = user(
            serde_json::json!("close the issues once it merges"),
            serde_json::json!({"origin": {"kind": "human"}, "promptSource": "typed",
                "turnOrigin": "human"}),
        );
        assert_eq!(
            typed,
            vec![session.clone(), Observed::UserTurn { at, wake: false }]
        );
        let wake = user(
            serde_json::json!(
                "<task-notification>\n<summary>Goal check-in: background work still running</summary>\n</task-notification>"
            ),
            serde_json::json!({"origin": {"kind": "task-notification"}, "promptSource": "system"}),
        );
        assert_eq!(wake, vec![session, Observed::UserTurn { at, wake: true }]);
    }
}
