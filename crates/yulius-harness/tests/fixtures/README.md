# Harness transcript fixtures

All content in this directory is **synthetic**. No line was copied from a real session,
transcript, credential or account. Only the record *structure* follows what was observed on
2026-10-07 from:

- Claude Code **2.1.292** session JSONL (`<CLAUDE_CONFIG_DIR>/projects/<cwd-slug>/<sessionId>.jsonl`);
- Codex CLI **0.160.1** rollout JSONL (`<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-<timestamp>-<uuid>.jsonl`).

The structure was supplied as a written description (field names, record types, nesting);
fixtures were not derived from private files. Live compatibility of the parsers and writers
with real CLI output is verified separately (SPEC-002 §11 "Live").

| File | Purpose |
| --- | --- |
| `claude/session.jsonl` | Two-turn session: thinking, text and `tool_use` lines sharing one `message.id` with repeated usage, a `tool_result` user record, a `system` record, an `isMeta` user record, a sidechain assistant record, and ignored record types (`file-history-snapshot`, `file-history-delta`, `mode`, `permission-mode`, `last-prompt`, `attachment`). |
| `claude/branches.jsonl` | A rewind: two user records share one parent; only the latest branch is the active conversation. |
| `claude/limit_epoch.jsonl` | Usage-limit API error (`isApiErrorMessage`) with an epoch reset after `\|`. |
| `claude/limit_text.jsonl` | Usage-limit API error with an `error` field and no parseable reset time, plus model text that merely mentions a limit (not an error). |
| `claude/absent_usage.jsonl` | An assistant message without `usage` (usage must stay unknown). |
| `claude/malformed.jsonl` | Truncated JSON, non-object, missing `type`, unknown type, missing `message`, non-JSON. |
| `codex/rollout.jsonl` | Two-turn rollout: `session_meta`, `turn_context`, developer and injected environment messages, reasoning, `function_call`/`function_call_output`, `custom_tool_call`/`custom_tool_call_output`, `agent_message`, `token_count` with cumulative usage and rate limits (including `info: null`), `task_started`/`task_complete`, `ordinal` fields and ignored types (`token_usage_record`, `compacted`, `world_state`, `thread_settings_applied`, `item_completed`). |
| `codex/rate_limit_reached.jsonl` | `rate_limit_reached_type` set with a full window, then with no full window. |
| `codex/malformed.jsonl` | Truncated JSON, missing payload, `session_meta` without id, unknown type, missing type, non-object. |

Reset times are epoch seconds (`1791380000` = 2026-10-07T13:33:20Z). Session ids, uuids,
message ids and hashes are made up.
