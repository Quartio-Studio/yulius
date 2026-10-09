//! Wire-parser contracts for Claude Code transcript lines (synthetic fixtures with the
//! structure of Claude Code 2.1.292).

use yulius_domain::time::Timestamp;
use yulius_domain::usage::TokenUsage;
use yulius_harness::claude;
use yulius_harness::observe::{LimitSignal, Observed, UsageAccumulator};

const CLAUDE_SESSION: &str = include_str!("fixtures/claude/session.jsonl");
const CLAUDE_LIMIT_EPOCH: &str = include_str!("fixtures/claude/limit_epoch.jsonl");
const CLAUDE_LIMIT_TEXT: &str = include_str!("fixtures/claude/limit_text.jsonl");
const CLAUDE_ABSENT_USAGE: &str = include_str!("fixtures/claude/absent_usage.jsonl");
const CLAUDE_MALFORMED: &str = include_str!("fixtures/claude/malformed.jsonl");

const CLAUDE_SESSION_ID: &str = "5b0d9c1e-2f4a-4c6b-9e7d-1a2b3c4d5e6f";

fn observe_all(text: &str, observe: fn(&str) -> Vec<Observed>) -> Vec<Vec<Observed>> {
    text.lines().map(observe).collect()
}

fn flat(per_line: &[Vec<Observed>]) -> Vec<&Observed> {
    per_line.iter().flatten().collect()
}

fn accumulate(text: &str, observe: fn(&str) -> Vec<Observed>) -> UsageAccumulator {
    let mut acc = UsageAccumulator::new();
    for line in text.lines() {
        acc.observe_all(&observe(line));
    }
    acc
}

#[test]
fn claude_session_counts_each_message_once_including_sidechains() {
    let acc = accumulate(CLAUDE_SESSION, claude::observe_line);
    // msg_01 (three lines, max per field) + msg_02 + sidechain + msg_03.
    assert_eq!(
        acc.usage(),
        TokenUsage {
            input: Some(120 + 5 + 50 + 7),
            output: Some(64 + 20 + 10 + 15),
            cache_read: Some(10_000 + 12_000 + 12_100),
            cache_write: Some(2_000 + 100),
        }
    );
    assert_eq!(acc.malformed_lines(), 0);
    assert_eq!(acc.model(), Some("claude-opus-4-1"));
}

#[test]
fn claude_session_turns_meta_and_ignored_records() {
    let per_line = observe_all(CLAUDE_SESSION, claude::observe_line);
    let all = flat(&per_line);
    let user_turns = all
        .iter()
        .filter(|o| matches!(o, Observed::UserTurn))
        .count();
    assert_eq!(
        user_turns, 2,
        "tool results, meta and sidechain records are not turns"
    );
    assert_eq!(
        all.first(),
        Some(&&Observed::SessionMeta {
            session_id: CLAUDE_SESSION_ID.into(),
            cwd: Some("/work/demo".into()),
            model: None,
            version: Some("2.1.292".into()),
        })
    );
    let msg01 = all
        .iter()
        .filter(|o| matches!(o, Observed::AssistantMessage { message_id, .. } if message_id == "msg_01SYNTH"))
        .count();
    assert_eq!(msg01, 3);
    // Unknown and bookkeeping record types yield nothing.
    for (line, observed) in CLAUDE_SESSION.lines().zip(&per_line) {
        let record: serde_json::Value = serde_json::from_str(line).unwrap_or_default();
        let kind = record.get("type").and_then(serde_json::Value::as_str);
        if !matches!(kind, Some("user" | "assistant")) {
            assert!(observed.is_empty(), "{line}");
        }
    }
    let completes = all
        .iter()
        .filter(|o| matches!(o, Observed::TurnComplete { duration_ms: None }))
        .count();
    assert_eq!(completes, 3, "end_turn of msg_02, sidechain and msg_03");
}

#[test]
fn claude_limit_with_epoch_reset() {
    let all: Vec<Observed> = CLAUDE_LIMIT_EPOCH
        .lines()
        .flat_map(claude::observe_line)
        .collect();
    let limits: Vec<&Observed> = all
        .iter()
        .filter(|o| matches!(o, Observed::LimitReached { .. }))
        .collect();
    assert_eq!(
        limits,
        vec![&Observed::LimitReached {
            resets_at: Some(Timestamp(1_791_380_000_000)),
            message: "Claude AI usage limit reached".into(),
            window_minutes: None,
        }]
    );
    // The synthetic error message is not counted as usage.
    assert!(
        !all.iter()
            .any(|o| matches!(o, Observed::AssistantMessage { .. }))
    );
    let acc = accumulate(CLAUDE_LIMIT_EPOCH, claude::observe_line);
    assert_eq!(
        acc.last_limit(),
        Some(&LimitSignal {
            resets_at: Some(Timestamp(1_791_380_000_000)),
            message: "Claude AI usage limit reached".into()
        })
    );
    assert!(acc.usage().is_unknown());
}

#[test]
fn claude_limit_without_parseable_time_and_model_text_is_not_a_limit() {
    let per_line = observe_all(CLAUDE_LIMIT_TEXT, claude::observe_line);
    // The model's own text mentioning a limit is an ordinary message.
    assert!(matches!(
        per_line[1].as_slice(),
        [
            Observed::AssistantMessage { .. },
            Observed::TurnComplete { .. }
        ]
    ));
    assert_eq!(
        per_line[3],
        vec![Observed::LimitReached {
            resets_at: None,
            message: "5-hour limit reached ∙ resets 3pm".into(),
            window_minutes: Some(300),
        }]
    );
}

#[test]
fn claude_absent_usage_stays_unknown() {
    let acc = accumulate(CLAUDE_ABSENT_USAGE, claude::observe_line);
    let usage = acc.usage();
    assert_eq!(usage.input, None);
    assert_eq!(usage.output, None);
    let per_line = observe_all(CLAUDE_ABSENT_USAGE, claude::observe_line);
    assert!(matches!(
        per_line[3].first(),
        Some(Observed::AssistantMessage { usage, .. }) if usage.is_unknown()
    ));
}

#[test]
fn claude_malformed_and_unknown_lines() {
    let per_line = observe_all(CLAUDE_MALFORMED, claude::observe_line);
    assert_eq!(
        per_line,
        vec![
            vec![Observed::Malformed], // truncated JSON
            vec![Observed::Malformed], // not an object
            vec![Observed::Malformed], // no type
            vec![],                    // unknown type is ignored
            vec![Observed::Malformed], // assistant without message
            vec![Observed::Malformed], // not JSON
        ]
    );
    assert_eq!(claude::observe_line(""), vec![Observed::Malformed]);
}
