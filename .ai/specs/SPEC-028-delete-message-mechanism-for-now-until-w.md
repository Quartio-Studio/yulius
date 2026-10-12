---
yulius-task: 2990b193-4021-4296-83bc-926f0b7dfbb7
status: done
---

# SPEC-028: remove the message mechanism for now

Status: implemented
Date: 2026-10-12
Owner: Yulius task #26
Scope authorization: user request (F 2026-10-12, task #26): "delete message mechanism for now until
we make the infra very good", with a screenshot of the tool rail pointing at **Messages**.

## Goal

Yulius has no messages until their delivery is reliable: no Messages panel, no unread counts, no
agent message tools, no stored notices, no notice digest and no wake-up typed into an idle session.
Delivery keeps working without them: an agent learns what happened to its checks, its delivery and
an armed merge from `yulius wait`, `ci_status` and `delivery_status`, which it already uses.

## Context

SPEC-012 removed task-to-task messages and kept "notices": Yulius's own messages to a task
(`tell_task` from delivery, armed merges and long-running shells; `tell_freed` from disk upkeep;
the intake's "Scope changed by the user"), read through `messages_inbox`, injected by the
`UserPromptSubmit` digest, blocking a turn end while urgent ones were unread, and typed into an idle
session as "Yulius: you have new messages…" (`background.rs`). The agent's `message_send
to="people"` showed in the Messages panel and in people's notifications. The typed wake-up had to be
fixed twice (Enter piled into the input box), and the panel mostly showed Yulius talking to agents.

What still tells the agent the same things without messages:

- checks at its head ended (and why): `yulius wait`, `ci_status`;
- its delivery moved (opened, merged, deployed): `yulius wait` (stage change), `delivery_status`;
- a pull request was opened by Yulius: the CI run it starts ends a running `yulius wait`;
- a step the user ruled out: the step is `skipped` with the note "Ruled out by the user: …", seen in
  every `steps_get`/`step_update` result and in the briefing;
- an armed merge decided: today `yulius wait` only says "Yulius's notice says how". It needs its own
  outcome (MSR-003).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| MSR-001 | The Yulius MCP server serves no `message_send` and no `messages_inbox`; calling either is an unknown-tool error. The server instructions, tool list in the briefing and the briefing itself do not mention messages, notices or an inbox. | `mcp_coordination.rs`, `briefing.rs` |
| MSR-002 | Yulius writes no message rows: delivery (pull request opened, checks failed, conflicts, behind), armed-merge outcomes, long-running background shells, freed disk space and the intake's scope change tell nobody through messages. | `delivery.rs`, `disk.rs`, `intake.rs` tests |
| MSR-003 | When a merge armed with `delivery_merge` is decided, `yulius wait` ends with how: merged (pull request and base), refused (with the reason) or dropped because the branch moved on. | `agent_wait` / `wait_cli` tests |
| MSR-004 | No hook injects a notice digest, the turn-end gate has no "urgent messages" reason, and nothing is typed into an idle session to wake it (the idle waker, its delay and its typed-text state are gone). | `agent_hooks.rs`; the waker is deleted from `background.rs` |
| MSR-005 | The web UI has no Messages: the tool rail is Browser, Editor and Shell; no unread badge on a task row or the rail; no notification for an agent message. `GET /api/tasks/{tid}/messages` is gone and the task view has no `unread_messages`. | e2e `rail`, `http_auth.rs` |
| MSR-006 | Stored messages stay (no migration) and the project export still carries them; the `wake_enabled` task column stays unused. | review |

Withdrawn by this spec: SPEC-001 COM-005 and the rest of AC-COM-2; SPEC-012 MSG-001, MSG-003,
MSG-004, MSG-005 (superseded); SPEC-019 SHL-002's "told once" (the UI still shows the shells' age);
SPEC-011 DLV-012's "a notice arrives" (replaced by MSR-003).

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Agents have no message tools: message_send, messages_inbox and the notice digest are gone
- [x] Yulius stores no notices and types no wake-ups; `yulius wait` says how an armed merge ended
- [x] The web UI has no Messages: rail tool, sidebar badge, notifications and the messages route are gone
- [x] Specs, ADR and docs say messaging is withdrawn for now
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-MSR-1: `tools/list` has neither message tool; `tools/call` of either is an error.
- AC-MSR-2: a failed check, an opened pull request, a freed cache and a scope change leave the task's
  message rows empty, and an idle task is never typed into.
- AC-MSR-3: a `yulius wait` running while an armed merge is merged, refused or dropped ends with a
  line that says which, and why for a refusal or a drop.
- AC-MSR-4: the rail shows Browser, Editor, Shell; `GET /api/tasks/{tid}/messages` answers 404/405.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- Integration (`crates/yulius/tests`): `mcp_coordination.rs` (tools gone), `delivery.rs` (armed merge
  outcome in the wait state; no message rows), `agent_hooks.rs` (no digest, no urgent gate),
  `supervision.rs` (the idle-wake test goes; the normal-exit part stays), `intake.rs` (scope change: steps skipped, no message),
  `http_auth.rs` (route gone).
- Unit: `briefing.rs` (no messages section), `wait_cli.rs` (settled line for each outcome).
- e2e: `rail.e2e.ts` (three tools), `messages.e2e.ts` deleted.

## Decisions

- Remove, not hide: a hidden panel over a live notice pipeline would keep the unreliable part (typed
  wake-ups, digest, stop gate) running unseen. ADR-0023.
- Pull, not push: agents already wait with `yulius wait` (a background command Claude Code wakes them
  from); its state gains the armed-merge outcome, held in memory per task like the armed merge itself
  (lost on restart, as the armed merge is).
- Keep the tables and the store API: a later return needs no migration, and the export keeps history.
- The scope-change notice is not replaced: the skipped steps carry the user's note.
- A merge being decided still reads as armed (a counter next to the armed merge), and every
  `delivery_merge` call forgets an earlier outcome: found by the independent review (round 1).

## Evidence

- `scripts/dev ci` exit 0 (fmt, clippy `-D warnings`, docs, repository check, 1151 tests passed,
  16 skipped) on the review-fix head; the last commit only tightens `wait_cli::settled` and is
  covered by its unit test, `cargo clippy -p yulius` and a final `scripts/dev ci` before merge.
- e2e: 129 passed, 1 skipped (full suite). One earlier full run had `intake.e2e.ts` fail on
  `__yuliusMock` not yet loaded; it passed 3 of 3 alone and the next full run. Filed as #246 (it
  races the mock's start; not this change).
- Tests that fail without the change: `mcp_coordination.rs` (tools gone, instructions),
  `agent_hooks.rs::stored_messages_reach_no_hook`, `delivery.rs::an_armed_merge_happens_when_the_checks_pass`
  (outcome, deciding gap), `an_armed_merge_says_why_it_was_dropped_or_refused`, `http_auth.rs`
  (route 404), `briefing.rs`, `wait_cli.rs`, e2e `rail.e2e.ts` (seen red before the UI change).
- Independent review (fresh read-only session, `om-code-review`): round 1 REQUEST CHANGES (2 major,
  fixed test-first), round 2 APPROVE fc1d339 with one minor (stale outcome), fixed test-first.
- Not covered here: the errored CI run retry kept in `delivery.rs` has no test (it had none before;
  #251). Not exercised: a real Claude Code session waking from `yulius wait` (unchanged mechanism).
