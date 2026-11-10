# Harness transcript fixtures

All content in this directory is **synthetic**. No line was copied from a real session,
transcript, credential or account. Only the record *structure* follows what was observed on
2026-10-07 from Claude Code **2.1.292** session JSONL
(`<CLAUDE_CONFIG_DIR>/projects/<cwd-slug>/<sessionId>.jsonl`), and, for `codex/`, the rollout
structure of Codex CLI **0.167.0** (`<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-*.jsonl`) as its
serde definitions in the `rust-v0.167.0` source describe it (SPEC-451; checked 2026-11-09).

The structure was supplied as a written description (field names, record types, nesting);
fixtures were not derived from private files. Live compatibility of the parsers with real
CLI output is verified separately (SPEC-002 §11 "Live").

| File | Purpose |
| --- | --- |
| `claude/session.jsonl` | Two-turn session: thinking, text and `tool_use` lines sharing one `message.id` with repeated usage, a `tool_result` user record, a `system` record, an `isMeta` user record, a sidechain assistant record, and ignored record types (`file-history-snapshot`, `file-history-delta`, `mode`, `permission-mode`, `last-prompt`, `attachment`). |
| `claude/limit_epoch.jsonl` | Usage-limit API error (`isApiErrorMessage`) with an epoch reset after `\|`. |
| `claude/limit_text.jsonl` | Usage-limit API error with an `error` field and no parseable reset time, plus model text that merely mentions a limit (not an error). |
| `claude/absent_usage.jsonl` | An assistant message without `usage` (usage must stay unknown). |
| `claude/goal.jsonl` | Goal mode (structure observed 2026-10-10 on Claude Code 2.1.292, read-only, field names and nesting only): `goal_status` attachments — the sentinel of a set goal, "not met" (the agent continues), "met" with iterations, duration and tokens, a sidechain one, "failed", and a sentinel marking a goal cleared — the `/goal` command's user records, the `stop_hook_summary` and `turn_duration` system records that close a turn, an `away_summary` recap and a `queued_command` task notification (a background shell that ended). |
| `claude/resumed.jsonl`, `claude/resumed/subagents/` | A session with two runs and a person's terminal on it (SPEC-091; record types, keys and nesting checked against Claude Code 2.1.292 transcripts on 2026-10-14, read-only, no content copied): run one (09:00–09:30Z) with thinking and `tool_use` lines sharing a `message.id`, a goal continuation (two `end_turn` answers in one turn), `stop_hook_summary` and `turn_duration` records, a background agent (`async_launched`) whose `turn_duration` with `pendingBackgroundAgentCount` waits for it, a `<task-notification>` wake-up, and an interrupted turn; run two (resumed 10:00–10:05Z) with a synchronous sub-agent; a terminal's turn at 11:00Z; plus `permission-mode`, `file-history-snapshot`, `last-prompt`, `ai-title` and `mode` records. The sub-agents' `agent-<id>.jsonl` (sidechain lines) and `.meta.json` sit in `resumed/subagents/`. |
| `codex/rollout.jsonl` | A paginated two-turn rollout (`ordinal` on every line): `session_meta`, `turn_context` per turn (two models), developer and `<environment_context>` messages (not prompts), typed user messages, `item_completed` events, reasoning, `exec_command` and `apply_patch` calls with outputs, `token_count` with usage and rate limits, one with `info: null`, one repeated line, `task_complete` with `duration_ms`, a `turn_aborted`, and ignored types (`token_usage_record`, `world_state`) plus `compacted`. |
| `codex/rate_limit_reached.jsonl` | `token_count` with `rate_limit_reached_type` set: a full primary window, then a limit with no full window. |
| `codex/malformed.jsonl` | Truncated JSON, a known type without payload, `session_meta` without id, an unknown type, no type, not an object. |
| `claude/malformed.jsonl` | Truncated JSON, non-object, missing `type`, unknown type, missing `message`, non-JSON. |

Reset times are epoch seconds (`1791380000` = 2026-10-07T13:33:20Z). Session ids, uuids,
message ids and hashes are made up.
