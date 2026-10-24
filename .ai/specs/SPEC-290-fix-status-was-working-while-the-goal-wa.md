---
yulius-task: 183b74e4-daf8-4087-bcff-3d1d480cdc35
status: in-progress
---

# SPEC-290: An achieved goal never leaves its task Working

Status: in-progress
Date: 2026-10-24
Owner: task #276
Scope authorization: task #276 ("fix, status was working while the goal was achieved", with a
screenshot of task #272).

## Goal

A running task whose session Claude Code recorded as "Goal achieved", with no background work
left, reads Done, whatever happened to Yulius's own Stop hook for that turn. It never reads
"Working" for hours after the goal was achieved, as task #272 did ("working 11h 15m").

## Context

- Symptom (task #272, 2026-10-24): the session's last turn was woken at 00:26:24 by the end of its
  `yulius wait` shell; its Stop hook ran at about 00:27:07 and Claude Code's goal evaluator
  recorded the goal as met at 00:27:11 (`goal_status` with `met: true`), then closed the turn
  (`turn_duration`, no background agent pending). The sidebar still showed the task Working at
  11:42, its working stretch counted from 00:26:24, until a person stopped it.
- The transcript's ending is the same as that of ~45 other goal tasks that read Done: the earlier
  Stops of the session got no verdict (Claude Code's evaluator waits while background work runs),
  the last turn was woken by a task notification, and the verdict followed its Stop hook.
- Live path (`crates/yulius-app/src/finish.rs`): the Stop hook's `clean_stop` opens the verdict
  slot and the verdict (`goal_observed`) or the turn's record (`turn_record_seen`) settles the
  turn. When the slot was never opened, `goal_observed(Met)` calls `settle_idle(None)`, which
  leaves a Working task as it is, and `turn_record_seen` does nothing either. The replay after a
  restart (`TranscriptFacts::fold`) treats the same verdict and record as the turn's end and
  settles a Working task.
- The hook client (`yulius-harness/src/hook_client.rs`) is fail-open: a Stop hook that Yulius
  answers with an error, or does not answer, prints nothing and Claude Code ends the turn. Nothing
  then settles it.
- Root cause: being confirmed by a reproduction of #272's ending
  (`task_done::a_goal_achieved_in_the_turn_a_shell_woke_makes_the_task_done`).

## Requirements

- GDW-001: a turn whose goal Claude Code recorded as achieved, closed by its turn record, leaves
  the running task Done (or Working with its background work counted), never Working with nothing
  running, including when Yulius never settled that turn's Stop.
- GDW-002: a turn Yulius's Stop hook kept going (a gate block, a hold, a switch) is not settled by
  this rule.

## Plan

- [ ] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review, findings fixed, delivered on green

## Acceptance criteria

- The regression test fails on `dev` and passes with the fix.
- `scripts/dev ci` passes.

## Test plan

- Integration test in `crates/yulius/tests/task_done.rs` with the fake agent replaying #272's
  ending: the goal turn leaves a background shell and gets no verdict, the shell's end wakes the
  session, the woken turn's verdict (achieved) comes seconds after its Stop hook.
- A test for the Stop hook that Yulius did not settle (failed open) followed by the achieved goal.

## Decisions

- Pending the root cause.
