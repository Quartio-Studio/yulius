//! Transcript reading, writing and cross-provider round trips (SPEC-002 §7.3, AC-ROT-4 unit
//! evidence). Synthetic fixtures; acceptance by the real CLIs is verified live separately.

use serde_json::Value;
use yulius_domain::harness::Harness;
use yulius_domain::transcript::{
    Block, Role, Transcript, TranslationLimits, Turn, plan_translation,
};
use yulius_harness::claude::{self, WriteParams};
use yulius_harness::codex::{self, RolloutParams};
use yulius_harness::observe::Observed;

const CLAUDE_SESSION: &str = include_str!("fixtures/claude/session.jsonl");
const CLAUDE_BRANCHES: &str = include_str!("fixtures/claude/branches.jsonl");
const CODEX_ROLLOUT: &str = include_str!("fixtures/codex/rollout.jsonl");

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn conversation_texts(transcript: &Transcript) -> Vec<(Role, String)> {
    transcript
        .turns
        .iter()
        .map(|t| (t.role, t.rendered_text()))
        .collect()
}

fn counter() -> impl FnMut() -> String {
    let mut n = 0u32;
    move || {
        n += 1;
        format!("00000000-0000-4000-9000-{n:012}")
    }
}

#[test]
fn claude_fixture_reads_into_ir() {
    let transcript = claude::read_transcript(CLAUDE_SESSION.lines());
    assert_eq!(transcript.meta.source, Harness::ClaudeCode);
    assert_eq!(
        transcript.meta.session_id,
        "5b0d9c1e-2f4a-4c6b-9e7d-1a2b3c4d5e6f"
    );
    assert_eq!(transcript.meta.cwd.as_deref(), Some("/work/demo"));
    assert_eq!(transcript.meta.model.as_deref(), Some("claude-opus-4-1"));
    let roles: Vec<Role> = transcript.turns.iter().map(|t| t.role).collect();
    assert_eq!(
        roles,
        vec![Role::User, Role::Assistant, Role::User, Role::Assistant]
    );
    assert_eq!(
        transcript.turns[1].blocks,
        vec![
            Block::Omitted {
                what: "thinking".into()
            },
            Block::Text("I'll add `greet` in src/lib.rs.".into()),
            Block::ToolCall {
                name: "Bash".into(),
                input_summary: "cargo test".into()
            },
            Block::ToolResult {
                summary: "test result: ok. 3 passed".into(),
                is_error: false
            },
            Block::Text("Tests pass. Added `greet`.".into()),
        ]
    );
    // isMeta injected context and the sidechain are not part of the conversation.
    assert_eq!(
        transcript.turns[2].blocks,
        vec![Block::Text("Now make it greet in Polish: cześć.".into())]
    );
    let all = conversation_texts(&transcript)
        .into_iter()
        .map(|(_, t)| t)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!all.contains("Sidechain"));
    assert!(!all.contains("Caveat"));
    assert!(transcript.turns[0].at.is_some());
}

#[test]
fn claude_rewind_keeps_only_the_active_branch() {
    let transcript = claude::read_transcript(CLAUDE_BRANCHES.lines());
    let texts: Vec<String> = conversation_texts(&transcript)
        .into_iter()
        .map(|(_, t)| t)
        .collect();
    assert_eq!(
        texts,
        vec![
            "first prompt",
            "first answer",
            "rewritten follow-up",
            "final answer"
        ]
    );
}

#[test]
fn claude_reader_falls_back_to_file_order_without_uuids_and_skips_garbage() {
    let lines = [
        r#"{"type":"user","sessionId":"s","message":{"role":"user","content":"q"}}"#,
        "garbage",
        r#"{"type":"assistant","message":{"id":"m","content":[{"type":"text","text":"a"},{"type":"image","source":{}}]}}"#,
    ];
    let transcript = claude::read_transcript(lines);
    assert_eq!(transcript.meta.session_id, "s");
    assert_eq!(
        transcript.turns[1].blocks,
        vec![
            Block::Text("a".into()),
            Block::Omitted {
                what: "image".into()
            }
        ]
    );
}

#[test]
fn codex_fixture_reads_into_ir() {
    let transcript = codex::read_transcript(CODEX_ROLLOUT.lines());
    assert_eq!(transcript.meta.source, Harness::Codex);
    assert_eq!(
        transcript.meta.session_id,
        "0199c1d2-3e4f-7a8b-9c0d-1e2f3a4b5c6d"
    );
    assert_eq!(transcript.meta.model.as_deref(), Some("gpt-5.1-codex"));
    assert_eq!(transcript.turns.len(), 4);
    assert_eq!(
        transcript.turns[0].blocks,
        vec![Block::Text("Add a greeting function and test it.".into())],
        "environment context and developer messages are dropped"
    );
    assert_eq!(
        transcript.turns[1].blocks,
        vec![
            Block::Omitted {
                what: "reasoning".into()
            },
            Block::ToolCall {
                name: "shell".into(),
                input_summary: "bash -lc cargo test".into()
            },
            Block::ToolResult {
                summary: "test result: ok. 3 passed\n".into(),
                is_error: false
            },
            Block::Text("Tests pass. Added `greet`.".into()),
        ],
        "agent_message does not duplicate the assistant message"
    );
    assert!(matches!(
        transcript.turns[3].blocks.first(),
        Some(Block::ToolCall { name, input_summary }) if name == "apply_patch" && input_summary.contains("cześć")
    ));
}

fn assert_text_preserved(source: &Transcript, round_tripped: &Transcript) {
    let expected: Vec<(Role, String)> = source
        .turns
        .iter()
        .map(|t| {
            let text: Vec<String> = t
                .blocks
                .iter()
                .filter_map(|b| match b {
                    Block::Text(text) => Some(text.clone()),
                    _ => None,
                })
                .collect();
            (t.role, text.join("\n\n"))
        })
        .collect();
    let actual = conversation_texts(round_tripped);
    assert_eq!(actual.len(), expected.len());
    for ((role, want), (got_role, got)) in expected.iter().zip(&actual) {
        assert_eq!(role, got_role);
        // Every original text of the turn survives, in order, inside the rendered turn.
        let mut rest = got.as_str();
        for piece in want.split("\n\n") {
            let at = rest.find(piece);
            assert!(at.is_some(), "missing {piece:?} in {got:?}");
            rest = &rest[at.unwrap_or_default() + piece.len()..];
        }
    }
}

#[test]
fn claude_to_codex_round_trip_keeps_all_text_in_order() -> TestResult {
    let source = claude::read_transcript(CLAUDE_SESSION.lines());
    let planned = plan_translation(&source, TranslationLimits::default());
    let rollout = codex::write_rollout(
        &planned,
        &RolloutParams {
            session_id: "0199ffff-0000-7000-8000-000000000001",
            cwd: "/work/demo",
            cli_version: "0.160.1",
            model: "gpt-5.1-codex",
            effort: Some("high"),
            start_ms: 1_791_364_320_000,
        },
    );
    // Every written line is a well-formed record the observer understands.
    for line in rollout.lines() {
        assert!(
            !codex::observe_line(line).contains(&Observed::Malformed),
            "{line}"
        );
    }
    let first: Value = serde_json::from_str(rollout.lines().next().unwrap_or_default())?;
    assert_eq!(first["type"], "session_meta");
    assert_eq!(first["payload"]["originator"], "yulius");
    assert_eq!(first["payload"]["model_provider"], "openai");
    assert_eq!(first["payload"]["cwd"], "/work/demo");
    let second: Value = serde_json::from_str(rollout.lines().nth(1).unwrap_or_default())?;
    assert_eq!(second["type"], "turn_context");
    assert_eq!(second["payload"]["model"], "gpt-5.1-codex");
    assert_eq!(second["payload"]["effort"], "high");
    let kinds: Vec<String> = rollout
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(|v| {
            let payload_type = v["payload"]["type"].as_str().unwrap_or_default().to_owned();
            format!(
                "{}:{}",
                v["type"].as_str().unwrap_or_default(),
                payload_type
            )
        })
        .collect();
    assert_eq!(
        kinds[2..],
        [
            "event_msg:task_started",
            "response_item:message",
            "response_item:message",
            "event_msg:task_complete",
            "event_msg:task_started",
            "response_item:message",
            "response_item:message",
            "event_msg:task_complete",
        ]
    );
    let back = codex::read_transcript(rollout.lines());
    assert_eq!(back.meta.session_id, "0199ffff-0000-7000-8000-000000000001");
    assert_text_preserved(&source, &back);
    assert!(
        conversation_texts(&back)[1]
            .1
            .contains("[tool Bash] cargo test")
    );
    assert!(
        conversation_texts(&back)[1]
            .1
            .contains("[thinking omitted]")
    );
    Ok(())
}

#[test]
fn codex_to_claude_round_trip_writes_a_uuid_chain() -> TestResult {
    let source = codex::read_transcript(CODEX_ROLLOUT.lines());
    let planned = plan_translation(&source, TranslationLimits::default());
    let mut ids = counter();
    let jsonl = claude::write_transcript(
        &planned,
        WriteParams {
            session_id: "5b0dffff-0000-4000-8000-000000000001",
            cwd: "/work/demo",
            version: "2.1.292",
            model: "claude-opus-4-1",
            git_branch: "feat/greeting",
            start_ms: 1_791_364_320_000,
            ids: &mut ids,
        },
    );
    let records: Vec<Value> = jsonl
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    assert_eq!(records.len(), 4);
    let mut parent = Value::Null;
    for (index, record) in records.iter().enumerate() {
        assert_eq!(record["parentUuid"], parent, "record {index}");
        assert_eq!(record["isSidechain"], false);
        assert_eq!(record["userType"], "external");
        assert_eq!(record["sessionId"], "5b0dffff-0000-4000-8000-000000000001");
        assert_eq!(record["cwd"], "/work/demo");
        assert_eq!(record["version"], "2.1.292");
        assert_eq!(record["message"]["content"][0]["type"], "text");
        parent = record["uuid"].clone();
    }
    assert_eq!(records[0]["type"], "user");
    assert_eq!(records[0]["timestamp"], "2026-10-07T09:12:00.000Z");
    assert_eq!(records[1]["timestamp"], "2026-10-07T09:12:00.001Z");
    assert_eq!(records[1]["type"], "assistant");
    assert_eq!(records[1]["message"]["model"], "claude-opus-4-1");
    assert_eq!(records[1]["message"]["stop_reason"], "end_turn");
    for line in jsonl.lines() {
        assert!(!claude::observe_line(line).contains(&Observed::Malformed));
    }
    let back = claude::read_transcript(jsonl.lines());
    assert_text_preserved(&source, &back);
    Ok(())
}

#[test]
fn writers_skip_empty_turns_and_handle_trailing_user_turn() {
    let transcript = Transcript {
        meta: yulius_domain::transcript::TranscriptMeta {
            source: Harness::ClaudeCode,
            session_id: "s".into(),
            cwd: None,
            model: None,
        },
        turns: vec![
            Turn {
                role: Role::Assistant,
                blocks: vec![Block::Text("leading assistant".into())],
                at: None,
            },
            Turn {
                role: Role::User,
                blocks: vec![],
                at: None,
            },
            Turn {
                role: Role::User,
                blocks: vec![Block::Text("trailing question".into())],
                at: None,
            },
        ],
    };
    let rollout = codex::write_rollout(
        &transcript,
        &RolloutParams {
            session_id: "x",
            cwd: "/w",
            cli_version: "0.160.1",
            model: "m",
            effort: None,
            start_ms: 0,
        },
    );
    let back = codex::read_transcript(rollout.lines());
    assert_eq!(
        conversation_texts(&back),
        vec![
            (Role::Assistant, "leading assistant".to_owned()),
            (Role::User, "trailing question".to_owned())
        ]
    );
    assert!(!rollout.contains("\"effort\""));
    let mut ids = counter();
    let jsonl = claude::write_transcript(
        &transcript,
        WriteParams {
            session_id: "s",
            cwd: "/w",
            version: "2.1.292",
            model: "m",
            git_branch: "b",
            start_ms: 0,
            ids: &mut ids,
        },
    );
    assert_eq!(jsonl.lines().count(), 2);
}

#[test]
fn oversized_conversation_is_bounded_before_writing() {
    let mut lines = Vec::new();
    for i in 0..400 {
        let kind = if i % 2 == 0 { "user" } else { "assistant" };
        let parent = if i == 0 {
            "null".to_owned()
        } else {
            format!("\"u{}\"", i - 1)
        };
        let message = if kind == "user" {
            format!(r#"{{"role":"user","content":"question {i}"}}"#)
        } else {
            format!(
                r#"{{"id":"m{i}","role":"assistant","content":[{{"type":"text","text":"answer {i}"}}]}}"#
            )
        };
        lines.push(format!(
            r#"{{"type":"{kind}","uuid":"u{i}","parentUuid":{parent},"sessionId":"s","message":{message}}}"#
        ));
    }
    let source = claude::read_transcript(&lines);
    assert_eq!(source.turns.len(), 400);
    let planned = plan_translation(&source, TranslationLimits::default());
    assert!(planned.turns.len() <= 300);
    let first = planned.turns[0].rendered_text();
    assert!(first.starts_with("[Yulius]"), "{first}");
    assert!(
        planned
            .turns
            .last()
            .is_some_and(|t| t.rendered_text() == "answer 399")
    );
}
