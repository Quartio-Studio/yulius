//! Wire-parser contracts for Claude Code transcript lines (synthetic fixtures with the
//! structure of Claude Code 2.1.292).

use yulius_domain::task::GoalOutcome;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::TokenUsage;
use yulius_harness::claude;
use yulius_harness::observe::{LimitSignal, Observed, UsageAccumulator};

const CLAUDE_SESSION: &str = include_str!("fixtures/claude/session.jsonl");
const CLAUDE_LIMIT_EPOCH: &str = include_str!("fixtures/claude/limit_epoch.jsonl");
const CLAUDE_LIMIT_TEXT: &str = include_str!("fixtures/claude/limit_text.jsonl");
const CLAUDE_ABSENT_USAGE: &str = include_str!("fixtures/claude/absent_usage.jsonl");
const CLAUDE_MALFORMED: &str = include_str!("fixtures/claude/malformed.jsonl");
const CLAUDE_GOAL: &str = include_str!("fixtures/claude/goal.jsonl");

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
    // The Claude Code that wrote the session's records (SPEC-461 CCM-011).
    assert_eq!(acc.cli_version(), Some("2.1.292"));
}

/// SPEC-461 CCM-011: a prompt record names the run's Claude Code through its session observation
/// (a resumed session continues on a newer one), a sub-agent's lines name nothing for the
/// session, and a record without a version leaves the last one known. Every other record of the
/// session's thread names it too: the next test.
#[test]
fn a_prompt_record_names_the_cli_version_through_its_session_observation() {
    let prompt = |version: Option<&str>| {
        let mut record = serde_json::json!({
            "type": "user",
            "sessionId": "s1",
            "cwd": "/work",
            "timestamp": "2026-11-11T10:00:00.000Z",
            "message": {"role": "user", "content": "go on"},
        });
        if let Some(version) = version {
            record["version"] = version.into();
        }
        record.to_string()
    };
    let mut acc = UsageAccumulator::new();
    assert_eq!(acc.cli_version(), None);
    acc.observe_all(claude::observe_line(&prompt(Some("2.1.311"))).iter());
    assert_eq!(acc.cli_version(), Some("2.1.311"));
    acc.observe_all(claude::observe_line(&prompt(None)).iter());
    assert_eq!(acc.cli_version(), Some("2.1.311"), "kept without a version");
    for side in claude::observe_line(&prompt(Some("9.9.9"))) {
        acc.observe_side(&side);
    }
    assert_eq!(acc.cli_version(), Some("2.1.311"), "a sub-agent's line");
    acc.observe_all(claude::observe_line(&prompt(Some("2.1.322"))).iter());
    assert_eq!(acc.cli_version(), Some("2.1.322"));
}

/// SPEC-461 CCM-011 (found with a real session's transcript): every record of the session's own
/// thread names the Claude Code that wrote it, so a session resumed with nothing typed names
/// its version before any prompt. The records have the structure of the first ones Claude Code
/// 2.1.322 and 2.1.311 wrote after a resume on 2026-11-11 (texts made up).
#[test]
fn every_record_of_the_sessions_own_thread_names_its_claude_code() {
    let resumed_meta = r#"{"parentUuid":"a1","isSidechain":false,"type":"user","message":{"role":"user","content":[{"type":"text","text":"Continue from where you left off."}]},"isMeta":true,"uuid":"u2","timestamp":"2026-11-11T11:56:37.389Z","turnPosition":{"promptIndex":3,"turnIndex":4},"userType":"external","entrypoint":"cli","cwd":"/w","sessionId":"s1","version":"2.1.322","gitBranch":"b","slug":"s"}"#;
    let interrupted_tool = r#"{"parentUuid":"a1","isSidechain":false,"promptId":"p1","type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"[Request interrupted by user for tool use]","is_error":true,"tool_use_id":"toolu_1"}]},"uuid":"u3","timestamp":"2026-11-11T11:28:40.573Z","toolUseResult":"Error: interrupted","toolDenialKind":"interrupted","sourceToolAssistantUUID":"a1","userType":"external","entrypoint":"cli","cwd":"/w","sessionId":"s1","version":"2.1.311","gitBranch":"b","slug":"s"}"#;
    let attachment = r#"{"parentUuid":"u2","isSidechain":false,"attachment":{"type":"remote_session_change","url":null,"commit":"c","pr":"p","sendUserFileHint":false,"managedCommit":false,"managedPr":false},"type":"attachment","uuid":"at1","timestamp":"2026-11-11T11:56:37.692Z","userType":"external","entrypoint":"cli","cwd":"/w","sessionId":"s1","version":"2.1.322"}"#;
    let answer = r#"{"parentUuid":"at1","isSidechain":false,"message":{"model":"claude-fable-5-2","id":"msg_2","type":"message","role":"assistant","content":[{"type":"thinking","thinking":"hm","signature":"x"}],"stop_reason":"tool_use","usage":{"input_tokens":4,"output_tokens":9}},"type":"assistant","uuid":"a2","timestamp":"2026-11-11T11:57:36.901Z","userType":"external","entrypoint":"cli","cwd":"/w","sessionId":"s1","version":"2.1.322"}"#;
    let turn_duration = r#"{"parentUuid":"p","isSidechain":false,"type":"system","subtype":"turn_duration","durationMs":453250,"messageCount":155,"timestamp":"2026-11-11T12:04:55.411Z","uuid":"u","isMeta":false,"userType":"external","entrypoint":"cli","cwd":"/w","sessionId":"s1","version":"2.1.322"}"#;
    let request = r#"{"type":"api-request","timestamp":"2026-11-11T11:56:37.725Z","sessionId":"s1","version":"2.1.322"}"#;
    for (line, version) in [
        (resumed_meta, "2.1.322"),
        (interrupted_tool, "2.1.311"),
        (attachment, "2.1.322"),
        (answer, "2.1.322"),
        (turn_duration, "2.1.322"),
        (request, "2.1.322"),
    ] {
        let (observed, named) = claude::observe_versioned(line);
        assert_eq!(named.as_deref(), Some(version), "{line}");
        assert_eq!(observed, claude::observe_line(line), "{line}");
    }
    // The resumed session's first record is no prompt: it starts no turn and names no session.
    assert_eq!(claude::observe_line(resumed_meta), []);

    // A sub-agent's record in the session's file, a record without a version and a line that
    // is no record name nothing.
    let side = answer.replace(r#""isSidechain":false"#, r#""isSidechain":true"#);
    assert_eq!(claude::observe_versioned(&side).1, None);
    let own = r#"{"type":"last-prompt","lastPrompt":"Fix the bug","sessionId":"s1"}"#;
    assert_eq!(claude::observe_versioned(own), (Vec::new(), None));
    assert_eq!(
        claude::observe_versioned("not json"),
        (vec![Observed::Malformed], None)
    );
    let untyped = r#"{"version":"2.1.322","sessionId":"s1"}"#;
    assert_eq!(
        claude::observe_versioned(untyped),
        (vec![Observed::Malformed], None)
    );

    // The run's version follows its lines: the latest one named, kept where none is.
    let mut acc = UsageAccumulator::new();
    for (line, version) in [
        (own, None),
        (interrupted_tool, Some("2.1.311")),
        (own, Some("2.1.311")),
        (resumed_meta, Some("2.1.322")),
        (side.as_str(), Some("2.1.322")),
    ] {
        let (observed, named) = claude::observe_versioned(line);
        if let Some(named) = named {
            acc.note_cli_version(&named);
        }
        acc.observe_all(observed.iter());
        assert_eq!(acc.cli_version(), version, "{line}");
    }
}

#[test]
fn claude_session_turns_meta_and_ignored_records() {
    let per_line = observe_all(CLAUDE_SESSION, claude::observe_line);
    let all = flat(&per_line);
    let user_turns = all
        .iter()
        .filter(|o| matches!(o, Observed::UserTurn { .. }))
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
        .filter(|o| {
            matches!(
                o,
                Observed::TurnComplete {
                    duration_ms: None,
                    ..
                }
            )
        })
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
            // When the record was written (SPEC-074 SUP-002).
            at: Some(Timestamp(1_791_360_001_000)),
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
            at: Some(Timestamp(1_791_360_003_000)),
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

/// TSK-014: Claude Code 2.1.292 records the goal's state as `goal_status` attachments and closes a
/// turn with `turn_duration` (`stop_hook_summary` is not an end: a blocked stop writes it too); the `/goal` command is a user prompt with its
/// time; a task notification ends its background work (SPEC-079); recaps and sidechain goals are
/// not observations.
#[test]
fn claude_goal_mode_records() {
    let per_line = observe_all(CLAUDE_GOAL, claude::observe_line);
    let relevant: Vec<&Observed> = flat(&per_line)
        .into_iter()
        .filter(|o| {
            !matches!(
                o,
                Observed::SessionMeta { .. }
                    | Observed::AssistantMessage { .. }
                    | Observed::AgentWork { .. }
            )
        })
        .collect();
    // Each with its record's time (SPEC-346 LPH-002).
    let goal = |outcome, at| Observed::Goal {
        outcome,
        at: Some(Timestamp(at)),
    };
    assert_eq!(
        relevant,
        vec![
            &goal(GoalOutcome::Set, 1_791_610_681_381),
            // The `/goal` command's echo starts no turn: the model's work after it does
            // (SPEC-085 STT-001).
            &Observed::TurnComplete {
                duration_ms: None,
                at: Some(Timestamp(1_791_610_700_000))
            },
            &goal(GoalOutcome::NotMet, 1_791_610_704_000),
            &Observed::TurnComplete {
                duration_ms: None,
                at: Some(Timestamp(1_791_610_729_402))
            },
            &goal(GoalOutcome::Met, 1_791_610_732_557),
            &Observed::TurnEnded {
                background_agents: None,
                at: Some(Timestamp(1_791_610_732_580)),
                duration_ms: Some(51_000),
            },
            &Observed::BackgroundEnded { id: "b1".into() },
            &goal(GoalOutcome::Failed, 1_791_611_400_000),
            &goal(GoalOutcome::Cleared, 1_791_611_460_000),
        ]
    );
    assert_eq!(
        accumulate(CLAUDE_GOAL, claude::observe_line).malformed_lines(),
        0
    );
}
