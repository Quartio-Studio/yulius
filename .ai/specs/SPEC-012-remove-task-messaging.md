---
yulius-task: 71572985-c2dd-4f3d-baf1-bfc91a282cee
status: done
---

# SPEC-012: remove task-to-task messaging for now

Status: implemented
Date: 2026-10-12
Owner: Yulius task #9
Scope authorization: user request (F 2026-10-12): "delete messaging-to-other-tasks feature for now".

## Goal

Agents no longer message other tasks, and nothing sends task-to-task messages automatically.
Yulius's own notices to a task, the inbox, the digest, the idle wake-up and the Messages panel
stay. The agent's message to people (`message_send` `to="people"`) stays.

## Context

`message_send` could address a task or broadcast (`to="all"`); `scope_claim` sent "Scope overlap"
messages to the other task; the briefing told agents to coordinate through `message_send`. Notices
from Yulius itself (`tell_task` for delivery, `tell_freed` for disk, the intake "Scope changed by
the user" notice) use the same messages tables and are unchanged. The tables and stored history
stay (no migration); old task-to-task rows may still show in a task's panel.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| MSG-001 | `message_send` accepts only `to="people"`; a missing `to`, a task reference or `"all"` is an error naming `people`. Kind `scope` is gone. | `mcp_coordination.rs` |
| MSG-002 | `scope_claim` reports overlaps only in the claiming agent's result; no message is sent to the other task and the advice to use `message_send` is gone. | `mcp_coordination.rs` |
| MSG-003 | The briefing, tool descriptions and server instructions describe `message_send` as for people and `messages_inbox` as notices from Yulius; the hook digest does not tell agents to reply to Yulius notices. | `briefing.rs`, `hooks.rs`, `agent_hooks.rs` |
| MSG-004 | The Messages panel, sidebar badge and kind badges describe notices and agent messages to people; the mock holds no task-to-task messages. | web e2e |
| MSG-005 | Notices from Yulius, `messages_inbox`, the digest and the idle wake-up behave as before. | `supervision.rs`, `agent_hooks.rs` |

## Plan

- [x] Tests first: message_send to people only, scope overlap without messages
- [x] Server: tool, scopes, briefing, digest
- [x] Web UI and mock
- [x] Specs and docs
- [x] Gates, review, delivery

## Acceptance criteria

- AC-MSG-1: `message_send` to a task or to `"all"` returns an error; `to="people"` still shows in the sender's thread and publishes an event; no task receives it.
- AC-MSG-2: overlapping `scope_claim` returns the overlap and creates no message.
- AC-MSG-3: a Yulius notice reaches `messages_inbox`, the digest and wakes an idle task.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

Integration tests in `mcp_coordination.rs`, `supervision.rs`, `agent_hooks.rs`; unit tests in
`briefing.rs`; e2e `messages.e2e.ts`, `rail.e2e.ts`.

## Decisions

- COM-002 and AC-COM-2 are withdrawn, not deleted: tables and history stay so a future return needs no migration.
- `task_get` and `tasks_list` stay: reading other tasks is not messaging.

## Evidence

- `scripts/dev ci` exit 0 (fmt, clippy, docs, repository check, tests).
- e2e: `messages` and `rail` pass. On a host at load average 25-29, `consistency`, `editor`, `filedrop`, `terminal` (and others) time out; the same four files fail identically on unmodified `dev` (eef36c5), so they are load timeouts, not this change.
