//! Wire-parser contracts for Claude Code transcript lines (synthetic fixtures with the
//! structure of Claude Code 2.1.292).

use yulius_domain::task::GoalOutcome;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{LimitScope, TokenUsage, UsageWindow};
use yulius_harness::observe::{LimitSignal, Observed, UsageAccumulator};
use yulius_harness::{antigravity, claude, codex};

const CLAUDE_SESSION: &str = include_str!("fixtures/claude/session.jsonl");
const CLAUDE_LIMIT_EPOCH: &str = include_str!("fixtures/claude/limit_epoch.jsonl");
const CLAUDE_LIMIT_TEXT: &str = include_str!("fixtures/claude/limit_text.jsonl");
const CLAUDE_ABSENT_USAGE: &str = include_str!("fixtures/claude/absent_usage.jsonl");
const CLAUDE_MALFORMED: &str = include_str!("fixtures/claude/malformed.jsonl");
const CLAUDE_GOAL: &str = include_str!("fixtures/claude/goal.jsonl");
const CODEX_ROLLOUT: &str = include_str!("fixtures/codex/rollout.jsonl");
const CODEX_REACHED: &str = include_str!("fixtures/codex/rate_limit_reached.jsonl");
const CODEX_MALFORMED: &str = include_str!("fixtures/codex/malformed.jsonl");
/// A rollout the real Codex CLI 0.169.0 wrote (`codex exec`, then `exec resume` that met a usage
/// limit) against a local stub of the Responses API; paths and instructions left out.
const CODEX_REAL_0_169: &str = include_str!("fixtures/codex/exec-0.169.0.jsonl");
/// The real Codex CLI 0.169.0 again, with a Stop hook that blocked the first turn's end once.
const CODEX_REAL_STOP_BLOCK: &str = include_str!("fixtures/codex/exec-0.169.0-stop-block.jsonl");
/// A transcript the real Antigravity CLI 1.5.10 wrote (`agy -p` three times on one conversation)
/// against a local stub of the Gemini API: two answered prompts, then a third that met the
/// stub's daily quota (429 `RESOURCE_EXHAUSTED`), retried eight times. Paths shortened to `/w`.
const AGY_REAL_QUOTA: &str = include_str!("fixtures/antigravity/quota-1.5.10.jsonl");
/// The real Antigravity CLI 1.5.10 again: a model step that called `run_command`, the tool step a
/// `PreToolUse` hook denied, and an invalid tool call.
const AGY_REAL_TOOL: &str = include_str!("fixtures/antigravity/tool-1.5.10.jsonl");
const CODEX_SESSION_ID: &str = "019a6b2e-7c1d-7f00-9a3e-1b2c3d4e5f60";

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

/// The agent writes its own transcript, and a line may be megabytes: the version a record names
/// is kept per run and shown, so only its first bytes are kept, cut on a character boundary;
/// whichever record names it. A version that is no plain `X.Y.Z` is kept as it is.
#[test]
fn the_version_a_transcript_names_is_kept_short() {
    use yulius_harness::observe::CLI_VERSION_BYTES;
    let mut acc = UsageAccumulator::new();
    acc.note_cli_version("2.2.0-beta.3+build.7");
    assert_eq!(acc.cli_version(), Some("2.2.0-beta.3+build.7"));

    let long = "9".repeat(4 * 1024 * 1024);
    let kept = format!("{}…", "9".repeat(CLI_VERSION_BYTES));
    acc.note_cli_version(&long);
    assert_eq!(acc.cli_version(), Some(kept.as_str()));
    // What was cut already stays as it is.
    acc.note_cli_version(&kept);
    assert_eq!(acc.cli_version(), Some(kept.as_str()));
    // 21 three-byte characters fit in 64 bytes; the ellipsis follows them.
    acc.note_cli_version(&"…".repeat(100));
    assert_eq!(acc.cli_version(), Some("…".repeat(22).as_str()));

    // A prompt record names it through its session observation: cut the same way.
    let prompt = serde_json::json!({
        "type": "user", "isSidechain": false, "sessionId": "s1", "cwd": "/w",
        "timestamp": "2026-11-11T10:00:00.000Z", "version": long,
        "message": {"role": "user", "content": "go on"},
    })
    .to_string();
    let mut acc = UsageAccumulator::new();
    acc.observe_all(claude::observe_line(&prompt).iter());
    assert_eq!(acc.cli_version(), Some(kept.as_str()));
    let mut acc = UsageAccumulator::new();
    let (_, named) = claude::observe_versioned(&prompt);
    acc.note_cli_version(&named.unwrap_or_default());
    assert_eq!(acc.cli_version(), Some(kept.as_str()));
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
            scope: LimitScope::Account,
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
    assert_eq!(acc.usage(), TokenUsage::UNKNOWN);
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
            scope: LimitScope::Account,
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
        Some(Observed::AssistantMessage { usage, .. }) if *usage == TokenUsage::UNKNOWN
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
            // The tool result where Claude Code takes what waits into the turn (SPEC-493
            // DEF-001).
            &Observed::Queue {
                change: yulius_domain::agent_cli::QueueChange::ToolResult,
                at: Some(Timestamp(1_791_610_684_362))
            },
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

/// SPEC-503 CDX-004: what the real Codex 0.169.0 wrote is read as the synthetic fixtures are:
/// its session and version, the two typed prompts (not its environment context), each response's
/// usage once (the line Codex repeats at the limit is the same response), its windows, the first
/// turn's end and the usage limit that ended the resumed turn, and no malformed line.
#[test]
fn a_rollout_the_real_codex_0_169_wrote_is_observed() {
    let all_lines = observe_all(CODEX_REAL_0_169, codex::observe_line);
    let all = flat(&all_lines);
    assert_eq!(
        all.first(),
        Some(&&Observed::SessionMeta {
            session_id: "01a1e623-342a-7bc2-9ca9-5b5c3d0d70c0".into(),
            cwd: Some("/w/t1".into()),
            model: None,
            version: None,
        })
    );
    let prompts = all
        .iter()
        .filter(|o| matches!(o, Observed::UserTurn { wake: false, .. }))
        .count();
    assert_eq!(
        prompts, 2,
        "Say hi and Go on; the environment context is not a turn"
    );
    let ends = all
        .iter()
        .filter(|o| matches!(o, Observed::TurnComplete { at: Some(_), .. }))
        .count();
    assert_eq!(ends, 1, "the resumed turn ended at the limit, not complete");
    let limits: Vec<&Observed> = all
        .iter()
        .copied()
        .filter(|o| matches!(o, Observed::LimitReached { .. }))
        .collect();
    match limits.as_slice() {
        [
            Observed::LimitReached {
                message, scope, at, ..
            },
        ] => {
            assert!(
                message.starts_with("You’ve hit your usage limit."),
                "{message}"
            );
            assert_eq!(*scope, LimitScope::Account);
            assert!(at.is_some());
        }
        other => panic!("one usage limit: {other:?}"),
    }
    let windows: Vec<(Option<UsageWindow>, Option<UsageWindow>)> = all
        .iter()
        .filter_map(|o| match o {
            Observed::RateLimits(u) => Some((u.primary, u.secondary)),
            _ => None,
        })
        .collect();
    assert_eq!(windows.len(), 2);
    for (primary, secondary) in windows {
        assert_eq!(primary.and_then(|w| w.used_percent), Some(42.5));
        assert_eq!(primary.and_then(|w| w.window_minutes), Some(300));
        assert!(primary.and_then(|w| w.resets_at).is_some());
        assert_eq!(secondary.and_then(|w| w.used_percent), Some(12.0));
        assert_eq!(secondary.and_then(|w| w.window_minutes), Some(10_080));
    }
    let acc = accumulate(CODEX_REAL_0_169, codex::observe_line);
    assert_eq!(
        acc.usage(),
        TokenUsage {
            input: Some(1_200 - 200),
            output: Some(30),
            cache_read: Some(200),
            cache_write: Some(0),
        }
    );
    assert_eq!(acc.malformed_lines(), 0);
}

/// SPEC-503 CDX-004: a resumed run counts only its own responses. Codex writes the session's
/// last usage again when the resumed run meets a usage limit before any response; counting that
/// would bill the earlier run's response to this one. The real 0.169.0 rollout's resumed run had
/// no response (its one request was refused), so it has no usage; the whole session has one.
#[test]
fn a_resumed_codex_run_counts_no_response_of_the_run_before()
-> Result<(), Box<dyn std::error::Error>> {
    let rollout = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/codex/exec-0.169.0.jsonl");
    let resumed = yulius_domain::time::parse_rfc3339("2026-11-16T20:54:00Z");
    let run = yulius_harness::recount::count_codex_run(&rollout, resumed, None)?
        .ok_or("the rollout is counted")?;
    assert_eq!(run.usage().output.unwrap_or(0), 0, "{:?}", run.usage());
    assert_eq!(run.usage().input.unwrap_or(0), 0, "{:?}", run.usage());
    let whole = yulius_harness::recount::count_codex_run(&rollout, None, None)?
        .ok_or("the rollout is counted")?;
    assert_eq!(whole.usage().output, Some(30));
    // Its `session_meta` names the Codex CLI, which is no Claude Code version (SPEC-461).
    assert_eq!(whole.cli_version(), None);
    Ok(())
}

/// SPEC-503 CDX-004: the text a blocking Stop hook makes Codex go on with is a user-role message
/// Codex writes itself (`<hook_prompt …>`, provenance `harness`): not a prompt a person typed.
/// Yulius's own Stop gate blocks that way, so reading it as a prompt would start a turn of its own.
#[test]
fn a_stop_hooks_continuation_is_not_a_typed_prompt() {
    let all_lines = observe_all(CODEX_REAL_STOP_BLOCK, codex::observe_line);
    assert!(
        CODEX_REAL_STOP_BLOCK.contains("<hook_prompt"),
        "the fixture holds the hook's continuation"
    );
    let prompts = flat(&all_lines)
        .iter()
        .filter(|o| matches!(o, Observed::UserTurn { .. }))
        .count();
    assert_eq!(prompts, 1, "only Say hi was typed");
}

/// SPEC-503 CDX-004: Codex counts both cached and newly cached tokens inside `input_tokens`
/// (`codex-api` `sse/responses.rs`: 100 = 40 cached + 60 written); each is counted once.
#[test]
fn codex_cache_writes_are_counted_once() {
    let line = r#"{"timestamp":"2026-11-16T20:53:49.332Z","type":"token_usage_record","payload":{"response_id":"resp_cw","usage":{"input_tokens":100,"cached_input_tokens":40,"cache_write_input_tokens":60,"output_tokens":5,"total_tokens":105}}}"#;
    let acc = accumulate(line, codex::observe_line);
    assert_eq!(
        acc.usage(),
        TokenUsage {
            input: Some(0),
            output: Some(5),
            cache_read: Some(40),
            cache_write: Some(60),
        }
    );
}

/// SPEC-503 CDX-004: a Codex 0.167 rollout gives its session, typed turns, their ends, the model,
/// each response's usage once and the account's rate limits.
#[test]
fn codex_rollout_turns_usage_and_rate_limits() {
    let per_line = observe_all(CODEX_ROLLOUT, codex::observe_line);
    let all = flat(&per_line);
    assert_eq!(
        all.first(),
        Some(&&Observed::SessionMeta {
            session_id: CODEX_SESSION_ID.into(),
            cwd: Some("/work/demo".into()),
            model: None,
            version: None,
        })
    );
    let prompts: Vec<Option<Timestamp>> = all
        .iter()
        .filter_map(|o| match o {
            Observed::UserTurn { at, wake: false } => Some(*at),
            _ => None,
        })
        .collect();
    assert_eq!(
        prompts.len(),
        2,
        "developer and environment-context messages are not turns"
    );
    assert!(prompts.iter().all(Option::is_some));
    let ends: Vec<Option<u64>> = all
        .iter()
        .filter_map(|o| match o {
            Observed::TurnComplete {
                duration_ms,
                at: Some(_),
            } => Some(*duration_ms),
            _ => None,
        })
        .collect();
    assert_eq!(ends, vec![Some(6100), Some(6100)]);
    assert!(
        all.iter()
            .any(|o| matches!(o, Observed::TurnInterrupted { at: Some(_) }))
    );
    assert!(all.iter().any(|o| matches!(o, Observed::Compacted)));
    assert!(
        all.iter()
            .any(|o| matches!(o, Observed::AgentWork { at: Some(_) }))
    );
    assert!(!all.iter().any(|o| matches!(o, Observed::Malformed)));

    let limits: Vec<UsageWindow> = all
        .iter()
        .filter_map(|o| match o {
            Observed::RateLimits(u) => u.primary,
            _ => None,
        })
        .collect();
    assert_eq!(
        limits.len(),
        4,
        "the repeated line reports its limits again"
    );
    assert_eq!(
        limits.last(),
        Some(&UsageWindow {
            used_percent: Some(47.0),
            window_minutes: Some(300),
            resets_at: Some(Timestamp(1_794_230_000_000)),
        })
    );

    let acc = accumulate(CODEX_ROLLOUT, codex::observe_line);
    // Each response once (the repeated line is the same response); cached tokens out of input.
    assert_eq!(
        acc.usage(),
        TokenUsage {
            input: Some((5_000 - 3_000) + (4_000 - 3_000)),
            output: Some(200 + 250),
            cache_read: Some(3_000 + 3_000),
            cache_write: Some(0),
        }
    );
    assert_eq!(acc.model(), Some("gpt-synthetic-codex-mini"));
    assert_eq!(acc.turns(), 2);
    assert_eq!(acc.last_limit(), None);
    assert!(acc.active().total_ms().is_some());
}

/// SPEC-503 CDX-004: a resumed run reads only its own lines, so it counts only its responses.
#[test]
fn codex_usage_of_a_resumed_run_is_its_own() {
    let second_turn: String = CODEX_ROLLOUT
        .lines()
        .skip_while(|l| !l.contains("\"turn_id\":\"2\""))
        .map(|l| format!("{l}\n"))
        .collect();
    let acc = accumulate(&second_turn, codex::observe_line);
    assert_eq!(acc.usage().output, Some(250));
}

/// SPEC-503 CDX-004: a reached rate limit marks the account's windows; the turn a usage limit
/// ended is the usage limit (its `task_complete` error), and another error is a turn's end.
#[test]
fn codex_rate_limit_reached() {
    let per_line = observe_all(CODEX_REACHED, codex::observe_line);
    let snapshots: Vec<_> = per_line
        .iter()
        .flatten()
        .filter_map(|o| match o {
            Observed::RateLimits(u) => Some(*u),
            _ => None,
        })
        .collect();
    assert_eq!(snapshots.len(), 2);
    // A full window: the limit resets with it.
    assert!(snapshots[0].limit_reached);
    assert_eq!(
        snapshots[0].limit_resets_at,
        Some(Timestamp(1_794_230_000_000))
    );
    // No full window: the latest known reset.
    assert!(snapshots[1].limit_reached);
    assert_eq!(
        snapshots[1].limit_resets_at,
        Some(Timestamp(1_794_700_000_000))
    );
    assert!(snapshots[0].exhausted(Timestamp(1_794_229_999_999), 99.0));

    assert_eq!(
        per_line[3],
        vec![Observed::LimitReached {
            resets_at: None,
            message: "You've hit your usage limit. Upgrade to Pro or try again at 1:46 PM.".into(),
            window_minutes: None,
            scope: LimitScope::Account,
            at: Some(Timestamp(1_794_218_404_000)),
        }]
    );
    assert_eq!(
        per_line[4],
        vec![Observed::TurnComplete {
            duration_ms: Some(10),
            at: Some(Timestamp(1_794_218_405_000)),
        }]
    );
    let acc = accumulate(CODEX_REACHED, codex::observe_line);
    assert!(acc.last_limit().is_some());
}

/// SPEC-503 CDX-004: malformed lines are reported and unknown types ignored, never fatal.
#[test]
fn codex_malformed_and_unknown_lines() {
    let per_line = observe_all(CODEX_MALFORMED, codex::observe_line);
    assert_eq!(
        per_line,
        vec![
            vec![Observed::Malformed], // truncated JSON
            vec![Observed::Malformed], // known type without payload
            vec![Observed::Malformed], // session_meta without id
            vec![],                    // unknown type is ignored
            vec![Observed::Malformed], // no type
            vec![Observed::Malformed], // not an object
        ]
    );
}

/// SPEC-505 AGY-004: the transcript the real agy 1.5.10 wrote gives the three typed prompts (not
/// the system messages it adds), each model response's tokens once (input without the cached
/// tokens, which it reports apart), the model, and a usage limit for each attempt of the daily
/// quota's 429; no line is malformed.
#[test]
fn a_transcript_the_real_agy_1_5_10_wrote_is_observed() {
    let all_lines = observe_all(AGY_REAL_QUOTA, antigravity::observe_line);
    let all = flat(&all_lines);
    let prompts = all
        .iter()
        .filter(|o| {
            matches!(
                o,
                Observed::UserTurn {
                    wake: false,
                    at: Some(_)
                }
            )
        })
        .count();
    assert_eq!(prompts, 3, "Say hello, Run the probe command, One more");
    let limits: Vec<&Observed> = all
        .iter()
        .copied()
        .filter(|o| matches!(o, Observed::LimitReached { .. }))
        .collect();
    assert_eq!(limits.len(), 8, "one per attempt the CLI recorded");
    match limits.first() {
        Some(Observed::LimitReached {
            message,
            scope,
            at,
            resets_at,
            window_minutes,
        }) => {
            assert!(message.contains("RESOURCE_EXHAUSTED"), "{message}");
            assert!(message.len() <= 256 + "…".len(), "shortened: {message}");
            assert_eq!(*scope, LimitScope::Account);
            assert!(at.is_some());
            assert_eq!(*resets_at, None);
            assert_eq!(*window_minutes, None);
        }
        other => panic!("a usage limit: {other:?}"),
    }
    let acc = accumulate(AGY_REAL_QUOTA, antigravity::observe_line);
    assert_eq!(
        acc.usage(),
        TokenUsage {
            input: Some(2 * 1_134),
            output: Some(2 * 66),
            cache_read: Some(2 * 100),
            cache_write: None,
        }
    );
    assert_eq!(acc.model(), Some("gemini-4-carbon-high"));
    assert_eq!(acc.malformed_lines(), 0);
    // Reading the lines again counts nothing twice.
    let mut twice = UsageAccumulator::new();
    for line in AGY_REAL_QUOTA.lines().chain(AGY_REAL_QUOTA.lines()) {
        twice.observe_all(&antigravity::observe_line(line));
    }
    assert_eq!(twice.usage(), acc.usage());
}

/// SPEC-505 AGY-004: a model step, a tool step and a refused tool call are the agent's work; the
/// typed prompt starts a turn; the system message is no one's work.
#[test]
fn an_agy_tool_turn_is_agent_work() {
    let per_line = observe_all(AGY_REAL_TOOL, antigravity::observe_line);
    let kinds: Vec<Vec<&str>> = per_line
        .iter()
        .map(|line| {
            line.iter()
                .map(|o| match o {
                    Observed::UserTurn { .. } => "prompt",
                    Observed::AgentWork { .. } => "work",
                    Observed::AssistantMessage { .. } => "message",
                    Observed::TurnContext { .. } => "model",
                    _ => "other",
                })
                .collect()
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            vec!["work"],
            vec!["model", "message", "work"],
            vec!["prompt"],
            vec![],
            vec!["model", "message", "work"],
            vec!["work"],
            vec!["model", "message", "work"],
        ]
    );
}

/// SPEC-505 AGY-004: what a plan or the API says at a limit is a limit of the subscription; a
/// per-minute rate limit the CLI waits out by itself is not; broken lines are counted, unknown
/// types ignored.
#[test]
fn agy_limits_and_broken_lines() {
    let line = |error: &str| {
        serde_json::json!({
            "step_index": 3, "source": "SYSTEM", "type": "ERROR_MESSAGE", "status": "DONE",
            "error": error, "created_at": "2026-11-17T17:09:21Z"
        })
        .to_string()
    };
    let limited = |error: &str| {
        antigravity::observe_line(&line(error))
            .iter()
            .any(|o| matches!(o, Observed::LimitReached { .. }))
    };
    assert!(limited("Individual quota reached. Resets in 3 days."));
    assert!(limited("baseline model quota reached"));
    assert!(limited("Your AI credits balance is too low to continue."));
    assert!(limited(
        "API error (attempt 1): Error 429, Message: Resource has been exhausted, Status: \
         RESOURCE_EXHAUSTED, Details: [quotaId:GenerateContentPaidTierInputTokensPerModelPerDay]"
    ));
    assert!(!limited(
        "API error (attempt 1): Error 429, Status: RESOURCE_EXHAUSTED, Details: \
         [quotaId:GenerateRequestsPerMinutePerProjectPerModel]"
    ));
    assert!(!limited(
        "API error (attempt 1): Error 503, Status: UNAVAILABLE"
    ));
    assert_eq!(
        antigravity::observe_line("{not json"),
        vec![Observed::Malformed]
    );
    assert_eq!(
        antigravity::observe_line(r#"{"step_index":1}"#),
        vec![Observed::Malformed]
    );
    assert!(
        antigravity::observe_line(
            r#"{"step_index":1,"source":"MODEL","type":"SOME_FUTURE_STEP","status":"DONE"}"#
        )
        .iter()
        .all(|o| matches!(o, Observed::AgentWork { .. }))
    );
    assert!(
        antigravity::observe_line(
            r#"{"step_index":1,"source":"SYSTEM","type":"SOME_FUTURE_STEP","status":"DONE"}"#
        )
        .is_empty()
    );
}

/// SPEC-505 AGY-004: a run's window of an Antigravity transcript counts its own steps, read by
/// their `created_at`.
#[test]
fn an_antigravity_run_counts_its_own_window() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("transcript.jsonl");
    std::fs::write(&path, AGY_REAL_QUOTA)?;
    let whole =
        yulius_harness::recount::count_antigravity_run(&path, None, None)?.ok_or("a count")?;
    assert_eq!(whole.usage().input, Some(2 * 1_134));
    // From the second prompt on (17:09:20): the second response only… both steps share the
    // second, so a window starting there keeps them; one starting at 17:09:21 keeps none.
    let late = yulius_domain::time::parse_rfc3339("2026-11-17T17:09:21Z").ok_or("time")?;
    let after = yulius_harness::recount::count_antigravity_run(&path, Some(late), None)?
        .ok_or("a count")?;
    assert_eq!(after.usage().input, None);
    let missing = dir.path().join("none.jsonl");
    assert!(yulius_harness::recount::count_antigravity_run(&missing, None, None)?.is_none());
    Ok(())
}
