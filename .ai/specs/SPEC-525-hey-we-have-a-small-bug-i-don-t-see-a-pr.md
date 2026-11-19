---
yulius-task: 425a5337-c7c6-4b92-95e4-0bd3bb4b3fc1
status: done
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
  in the reader repairs them together; the index had already passed those sessions with no
  passages, so a migration indexes them again.
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
- [x] Independent review in a fresh session, findings fixed test-first

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
- **Checked on the real shapes, not only the fixtures** (review round 1, the reviewer's survey of
  this host's 22 Codex 0.169 rollouts and 3 Antigravity 1.5.10 transcripts): Codex runs every shell
  command from a code-mode `exec` script (`text(await tools.exec_command({cmd:`…`}))`) whose output
  parts wrap each result in JSON (`{"chunk_id", "exit_code", "output", …}`); `shell_command` is the
  bundled models' shell; Antigravity names files `Path` (`read_file`, `edit_file`, `write_file`)
  and numbers `read_file`'s lines `1: `. A free-text call is tracked as a dotenv read when its
  script names a dotenv file; a wrapper's `output` is the text redacted; `redact_env` accepts the
  `1: ` numbering and a patch's `+`/`-` marker (in dotenv content only: a diff in other text is no
  dotenv block). `AbsolutePath` and `TargetFile` stay tracked too: no real call used them, but a
  key tracked for nothing costs nothing.
- **Codex's injected-context prefixes live in `yulius-domain`** (`CODEX_INJECTED_CONTEXT_PREFIXES`),
  read by the rollout observer and the transcript reader alike: a prompt without provenance is
  shown quietly only when it opens as Codex's own context does, so a typed `<div> …` stays a prompt
  (`render_text` and the index drop meta blocks).
- **Code-mode results are read out at any depth, and continuations are tracked** (review round 2):
  a script prints results nested (`Promise.allSettled` lists, `{status, value}`, lists of wrappers
  or strings, an MCP tool's `{content, isError}`), so every `output`, `content[].text`, `reason` and
  list string is read out as lines; JSON with none of them is kept, its strings redacted as dotenv
  content when the call read a dotenv file. A tracked call that yields (`Script running with cell
  ID n`, read by `wait`) or leaves a command running (`session_id`, read by `write_stdin`) passes
  its tracking on; an Antigravity command on a dotenv file passes it to later `manage_task` reads
  (over-redaction on the safe side). Escapes in a script separate words; a function-style
  `apply_patch` (`{"input": …}`) and a path one level down (`call_mcp_tool`'s `Arguments`) are read.
- **Antigravity background results and loose session ids** (review round 3): a command that goes
  on as a task reports its result as a system message (`Task id "…" finished with result: …`),
  redacted as dotenv content once a command on a dotenv file ran; a script that calls
  `write_stdin` continues any tracked session id it names, however (`[7, 9].map(session_id => …)`,
  a variable). JSON is read out only when it is a code-mode result (a `chunk_id`, a settled
  promise, an MCP `{content, isError}`), with a settled `value` shown; other JSON is shown whole.
- **Sessions the old reader passed are indexed again** (schema version 69): the index had recorded
  Codex and Antigravity sessions as indexed with no passages; migration 0069 resets their index so
  search finds them, Claude Code sessions keep theirs.
- **Pre-existing, filed:** #1457 (Claude Code's `BashOutput` of a background dotenv read),
  #1453 (the index's dotenv tracking restarts each batch), #1454 (a Codex
  session document can keep a `missing-<id>.jsonl` path), #1455 (assistant entries are announced as
  "Claude").
- **A Codex message Codex added is shown quietly, not hidden.** Its provenance (`harness`) says so
  when present; otherwise a text opening with one of Codex's injected-context prefixes
  (`<environment_context>`, `<hook_prompt …>`, `# AGENTS.md instructions`, …) does: shown as a meta block, as Claude Code's
  `<system-reminder>` is. Developer messages (Codex's own instructions) are left out.

## Evidence

- **Reproduced (red before the fix):** `transcripts::tests::a_codex_rollout_reads_as_prompts_answers_and_tool_calls`,
  `an_antigravity_transcript_reads_as_requests_answers_and_tool_steps` and
  `a_dotenv_read_by_codex_or_antigravity_is_redacted_on_every_page` read zero entries at commit
  216e14c61; `task_transcript::a_watcher_reads_a_codex_task_s_rollout` (the watch view's request,
  `GET /api/tasks/{tid}/transcript`) returned the session id with `entries: []`.
- **Fixed:** the same tests pass. Each review round's findings got tests that failed first:
  4495fe469 (round 1: real CLI shapes), c9fba3c33 (round 2: nested, escaped, continued reads,
  migration 0069), 7f1e7f752 (round 3: Antigravity background results, `write_stdin` by any id,
  result display) and the round-4 test (settled content items, cancelled results).
- **Local checks:**
  - `cargo nextest run -p yulius-app -p yulius-store -p yulius-domain -p yulius-harness --lib`:
    1,413 passed.
  - `cargo nextest run -p yulius-harness`: 239 passed.
  - `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^(task_transcript|sessions|session_sync|antigravity|codex)/)'`:
    55 passed.
  - `cargo clippy … -- -D warnings`: clean.
  - `scripts/dev quick`: passed.
- **Real shapes:** the reviewers' probes (outside the repository, synthetic values) ran over the
  structures of this host's 23 Codex 0.169 rollouts and Antigravity 1.5.10 transcripts. All 28
  leak cases print "hidden". Paging with a kept cursor, without one, and the tail give identical
  entries on every real file. Base against head on 1,142 Claude Code transcripts (716k lines)
  differs on 2 lines, both redacting more in a dotenv-tracked result.
- **Independent review:** round 1 (Fable 5.2) and round 2 (Fable 5.2) asked for the dotenv cases on
  the real shapes; round 3 (Opus 5.5) asked for the Antigravity background result; round 4 (Opus
  5.5) approved d53670f6 with one minor (settled content items) and one nit (`with result`), fixed
  after it within the approval's 20 product lines (7 changed).
- **Security scan:** Yulius CI's `security` job was skipped (exit 78: Vidoc not run on this
  instance), which is unrun, not passed.
- **Not tested here:** the watch view against a live Codex or Antigravity run in a browser. The
  web view needed no change (it renders entries whatever their harness), and the endpoint test
  covers the request it makes.
- **Pre-existing, filed:** #1453, #1454, #1455, #1457.
