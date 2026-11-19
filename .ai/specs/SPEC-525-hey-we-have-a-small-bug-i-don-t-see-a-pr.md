---
yulius-task: 425a5337-c7c6-4b92-95e4-0bd3bb4b3fc1
status: in-progress
---

# SPEC-525: watchers see Codex and Antigravity sessions

## Goal

A person watching someone else's Codex or Antigravity task sees its session as the session
writes it, as they already do for a Claude Code task. Today the watch view stays on "Codex is
starting… / Antigravity is starting… The session appears here in a moment." for the whole run
(the owner's screenshots of #502 and #503, 2026-11-19: "I don't see a preview from codex and
antigravity when it's Michal's task, in opus it works good"). Fixes #1440.

## Context

- **Symptom.** A task owned by someone else is shown as its live transcript, not a terminal
  (SPEC-179): `watchview.js` reads `GET /api/tasks/{tid}/transcript` and shows
  `emptyTerminalMessage(task)` while the page has no entries. A running task's message is
  "`<CLI>` is starting…".
- **Root cause.** `App::task_transcript` finds the right file for each harness
  (`native_transcript`: Claude's session JSONL, the Codex rollout, the Antigravity
  `transcript.jsonl`), but every reader in `crates/yulius-app/src/transcripts.rs` (`read_tail`,
  `read_page_at`, `read_for_index`) maps lines with `entry_of`, which only knows Claude Code's
  records (`type` = `user`, `assistant`, `system`, `summary`). A Codex rollout line has `type`
  `session_meta`, `response_item`, `event_msg`, … and an Antigravity step has `type`
  `USER_INPUT`, `PLANNER_RESPONSE`, … so each maps to `None`: every page is empty, for as long as
  the task runs.
- **Why the tests missed it.** The transcript tests (`transcripts.rs` unit tests,
  `tests/task_transcript.rs`) use Claude Code lines only; SPEC-503 and SPEC-505 tested the
  harnesses' observers (turns, tokens, limits) on their own transcripts, not the reader people see.
- **What else the cause breaks.** The same reader serves the Sessions page of a task's session
  (`GET /api/sessions/{id}/transcript`), session search (the index reads `read_for_index`) and
  `session_read` for agents: a Codex or Antigravity task's session is empty in all of them. The fix
  in the reader repairs them together.
- **The formats.** Codex rollouts (`crates/yulius-harness/tests/fixtures/codex/`, Codex CLI
  0.167–0.169): `response_item` payloads are `message` (roles `developer`, `user`, `assistant`;
  `input_text`/`output_text` parts; a user message Codex added is marked by provenance `harness`
  or starts with a tag such as `<environment_context>`), `reasoning` (`summary` of
  `summary_text`), `function_call` (`name`, `arguments` as a JSON string, `call_id`),
  `function_call_output` (`call_id`, `output`), `custom_tool_call` (`input` text, e.g. an
  `apply_patch`) and `custom_tool_call_output`. `event_msg` lines repeat these items or carry
  counters. Antigravity steps (`fixtures/antigravity/`, agy 1.5.10): `step_index`, `source`,
  `type`, `status`, `created_at`, `content` or `error`; `USER_INPUT` wraps the person's text in
  `<USER_REQUEST>`; `PLANNER_RESPONSE` has `content` and `tool_calls` (`name`, `args`); other
  `MODEL` steps (tool runs) and `ERROR_MESSAGE` steps carry `content`/`error`.

## Requirements

| ID | Requirement |
| --- | --- |
| WCX-001 | A Codex rollout line reads as an entry: a typed user message as a user text, a message Codex added as a quiet meta block, developer messages not at all, assistant text, a reasoning summary as thinking, a function or custom tool call as a tool call (its id the `call_id`, its input the parsed arguments, a one-line summary of the command or patch), and its output as that call's result. Other record types (`event_msg`, `turn_context`, `session_meta`, counters) are not entries. |
| WCX-002 | An Antigravity step reads as an entry: a `USER_INPUT` as the person's request (the text inside `<USER_REQUEST>`, else the whole content), a `PLANNER_RESPONSE` as the assistant's text and tool calls, a system message as a quiet system entry, a model API error as an error result, other model steps (tool runs) as a tool result. |
| WCX-003 | Redaction is the same as for Claude Code: every text through `yulius_domain::redact`, secret-named JSON keys redacted, and the result of a Codex or Antigravity command that reads a dotenv file redacted as dotenv content, also when the page starts after that call. |
| WCX-004 | Claude Code transcripts read exactly as before. |

## Plan

- [x] Root cause found and the defect filed as an issue
- [x] Spec SPEC-525 written and valid
- [x] Codex and Antigravity transcripts read as entries, regression tests first
- [x] Docs and the changes log updated
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- AC-1 (WCX-001): `read_tail` of the Codex fixture rollouts returns the typed prompts, the
  assistant's answers, the tool calls with their results, and no developer instructions as
  prompts.
- AC-2 (WCX-002): `read_tail` of the Antigravity fixtures returns the requests without their
  metadata wrapper, the responses, the tool call and the errors.
- AC-3 (WCX-003): a dotenv value printed by a Codex `exec_command` or an Antigravity
  `run_command` that read `.env` is redacted, from the start and from a page after the call.
- AC-4 (WCX-001, end to end): `GET /api/tasks/{tid}/transcript` of a Codex task whose rollout
  holds a prompt and an answer returns them (the request the watch view makes).
- AC-5 (WCX-004): the existing Claude Code transcript tests pass unchanged.

## Test plan

- Unit (`crates/yulius-app/src/transcripts.rs`): the fixture rollouts and transcripts of
  `yulius-harness/tests/fixtures` read as entries (fails before the fix: zero entries); dotenv
  redaction across pages for both formats.
- Integration (`crates/yulius/tests/task_transcript.rs`): a Codex task's transcript endpoint
  returns its entries.
- The web view needs no change: it renders entries whatever their harness (`transcriptEntries`).

## Decisions

- **One reader, the format told by the line.** `entry_of` tells a Codex line (a `payload` object)
  and an Antigravity step (`step_index` and `source`) from a Claude Code record by its shape and
  maps each to the same `TranscriptEntry`. Rejected: passing the harness down to every reader
  (`read_tail`, `read_page_at`, `read_for_index` and their callers, the session-doc index): more
  plumbing for the same result, since the three formats cannot be mistaken for each other.
- **No tool-result pairing for Antigravity.** Its steps name no call id, so a tool run's step is a
  result of its own (id `step-<n>`) shown after its call, not folded into it.
- **A Codex message Codex added is shown quietly, not hidden.** Its provenance (`harness`) says so
  when present; otherwise a text starting with a tag (`<environment_context>`, `<hook_prompt …>`)
  or Codex's `# AGENTS.md instructions` heading does: shown as a meta block, as Claude Code's
  `<system-reminder>` is. Developer messages (Codex's own instructions) are left out.
