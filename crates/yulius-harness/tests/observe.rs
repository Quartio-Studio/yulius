//! Wire-parser contracts for Claude Code transcript lines (synthetic fixtures with the
//! structure of Claude Code 2.1.292).

use yulius_domain::task::GoalOutcome;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{TokenUsage, UsageWindow};
use yulius_harness::observe::{LimitSignal, Observed, UsageAccumulator};
use yulius_harness::{claude, codex};

const CLAUDE_SESSION: &str = include_str!("fixtures/claude/session.jsonl");
const CLAUDE_LIMIT_EPOCH: &str = include_str!("fixtures/claude/limit_epoch.jsonl");
const CLAUDE_LIMIT_TEXT: &str = include_str!("fixtures/claude/limit_text.jsonl");
const CLAUDE_ABSENT_USAGE: &str = include_str!("fixtures/claude/absent_usage.jsonl");
const CLAUDE_MALFORMED: &str = include_str!("fixtures/claude/malformed.jsonl");
const CLAUDE_GOAL: &str = include_str!("fixtures/claude/goal.jsonl");
const CODEX_ROLLOUT: &str = include_str!("fixtures/codex/rollout.jsonl");
const CODEX_REACHED: &str = include_str!("fixtures/codex/rate_limit_reached.jsonl");
const CODEX_MALFORMED: &str = include_str!("fixtures/codex/malformed.jsonl");
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

/// SPEC-451 CDX-004: a Codex 0.167 rollout gives its session, typed turns, their ends, the model,
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
            version: Some("0.167.0".into()),
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

/// SPEC-451 CDX-004: a resumed run reads only its own lines, so it counts only its responses.
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

/// SPEC-451 CDX-004: a reached rate limit marks the account's windows; the turn a usage limit
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

/// SPEC-451 CDX-004: malformed lines are reported and unknown types ignored, never fatal.
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
