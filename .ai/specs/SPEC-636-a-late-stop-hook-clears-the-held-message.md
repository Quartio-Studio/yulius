---
yulius-task: ddb1cc17-40cd-47c9-8308-27b33f3f0d26
status: in-progress
---

# SPEC-636 A late Stop of a turn its record settled keeps a flag raised since

## Goal

A Stop hook that reaches Yulius after the transcript record already settled its turn does not
clear a Needs-your-action flag raised after that settle, such as the held-message flag of
SPEC-480 DRF-006. Fixes #1615.

## Context

`task_message::a_draft_the_screen_shows_holds_until_it_is_cleared` failed once under host load
(#1615, production instance at `52b59ecb3`): after Ctrl+U, the watch half a minute later found
the held-message flag gone (`Null`) although the keyboard minute still held the message.

### Root cause

1. The fake agent, like Claude Code, writes the turn's closing transcript record before it runs
   its Stop hook (`tools/fake-agent/src/main.rs`, `echo` → `t.turn` then `hooks.stop()`).
2. The transcript watcher reads the record first. No Stop is in flight yet and none was answered
   since the turn began, so `closes_unanswered_turn` holds and `settle_unanswered_turn`
   (`crates/yulius-app/src/finish.rs`) settles the turn: the task reads Waiting and
   `last_stop_ms` is the record's time (SPEC-290 GDW-001).
3. The test's `idle_full_screen` returns on that Waiting. A person types a draft, and the MCP
   watch raises the held-message flag (`tell_of_held_message`: Needs your action).
4. The delayed Stop now arrives. `clean_stop` → `settle_clean_turn` re-reads the task (Needs your
   action) and, in prompt mode, calls `set_task_activity(Waiting)`, which overwrites the flag.
5. After Ctrl+U the box is held as "at the keyboard" for a minute (`BoxHeld::AtKeyboard`), which
   refuses without telling anyone, so nothing raises the flag again until the hold expires.

The Stop answers a turn that was already settled; the flag it clears belongs to a later matter.
Tests missed it because the Stop normally reaches Yulius within milliseconds of the record,
before any person or watch can act.

### What else the cause can break

Any flag raised between a record's settle and its late Stop (a held or visible-draft message, a
Yulius flag, Claude Code's idle notice) is cleared by that Stop in prompt mode. Goal mode already
keeps Needs your action when the goal is not achieved.

## Requirements

- LSK-001: A clean Stop whose turn was settled since it began by no answered Stop (by its record)
  keeps a Needs-your-action flag the task reads; it still ends the turn and runs the turn-end
  background work.
- LSK-002: A Stop of a turn not settled yet clears Needs your action as before (prompt mode).
- LSK-003: The held-message flag survives a cleared box during the keyboard minute and ends once
  the box is seen empty after it (SPEC-572 AC2), whatever order the record and the Stop come in.

## Plan

- [ ] Spec with the root cause of the late Stop
- [ ] Deterministic regression: a Stop held past the held-message flag
- [ ] A Stop of an already-settled turn keeps a flag raised since
- [ ] Docs, ADR and changes entry
- [ ] Independent review approves the head

## Acceptance criteria

- The new integration test holds the first Stop at `race::Point::StopTurnLock` until the flag was
  raised and Ctrl+U sent; it fails (`Null`) without the fix and passes with it.
- `task_message` passes in full; the unit tests of the settled-turn predicate pass.
- Yulius CI is green at the reviewed head.

## Test plan

- Unit: `stop_of_settled_turn` for a record-settled turn, an answered one, a new turn and an
  unknown begin.
- Integration (`crates/yulius/tests/task_message.rs`): the draft test with the opening Stop held
  until after the flag and Ctrl+U; the existing test unchanged.
- Mutation by hand: drop the guard, watch the new test fail.

## Decisions

- Product fix, not only a test wait: waiting in `idle_full_screen` for the answered Stop would
  hide a real ordering Claude Code produces (record before Stop), and a person's flag would still
  vanish in production. The test keeps no extra wait.
- The settled turn is recognized from existing facts: `last_stop_ms >= turn_began_ms` (settled
  since the turn began) and `stop_answered_ms < turn_began_ms` (no Stop of it was answered). Only
  `settle_unanswered_turn` settles a turn that way; a turn whose begin is unknown (0) is not
  judged settled. No new runtime field.
- The guard only keeps Needs your action; every other part of the late Stop (ending the turn,
  probes, chains, pull request) runs as before.
