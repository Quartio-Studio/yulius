---
yulius-task: d70cfa55-33e7-4766-8500-30e1e3d2717d
status: in-progress
---

# SPEC-301: a run lost while Yulius runs saves its live screen

Status: in-progress
Date: 2026-10-24
Owner: Yulius task #287
Scope authorization: issue [#686](https://github.com/Quartio-Studio/yulius/issues/686) (found by
the independent review of SPEC-229, task #215): "When the hub has output, `mark_lost` saves it with
`save_screen`, so the terminal shows the run's real last screen." Acceptance: a run lost while the
server is up with output in its hub has that output in its `final-screen`; a hub with no output does
not overwrite the saved screen; a regression test covers the lost-connection path. Refines
[SPEC-229](SPEC-229-this-can-t-be-this-way-something-got-fuc.md) RBT-002 and
[ADR-0177](../../docs/decisions/0177-a-run-lost-after-its-goal-closes-the-task.md).

## Goal

A task whose run was lost shows the run's real last screen, not the screen its last settled turn
left, whenever the server still held that screen.

## Context

- Symptom: a run lost while Yulius is up (a lost connection, `resolve_lost_connection`; a stop or
  close whose escalation timed out, `arm_escalation`) keeps the `final-screen` its last settled
  turn saved. Output written after that turn is gone once the task's terminal is restored from the
  file (after a restart, or any runtime whose hub is empty).
- Root cause: `Supervisor::mark_lost` (`crates/yulius-app/src/supervisor.rs`) records the attempt
  `Lost`, settles the task and ends the hub, but never calls `save_screen`. Only
  `finalize_attempt` (an observed exit), a settled turn (`set_task_activity`) and an idle
  reattach save the screen (SPEC-229 RBT-002).
- Every path into `mark_lost` misses the save: `resolve_lost_connection` (directly, or through
  `terminate_then_mark_lost`), `arm_escalation` (through `terminate_then_mark_lost`) and
  reconciliation's server-lost branches. In reconciliation the runtime is new and its hub has no
  output, so `save_screen` keeps the file there.
- Why the tests missed it: the SPEC-229 reboot test kills the processes while no server runs, so
  its loss is recorded by a new server with an empty hub; no test checked the screen of a run lost
  while the server was up.

## Requirements

- LRS-001: When `mark_lost` records a loss and the task's runtime exists, it saves the hub's screen
  to the attempt's `final-screen` with `save_screen` before the attempt becomes `Lost` (as
  `finalize_attempt` does before its transition), so no reader sees the run ended with the old file.
- LRS-002: A hub without output never overwrites the saved screen (`save_screen`'s existing rule);
  a deleted task's runtime is not brought back to save one.

## Plan

- [x] Spec SPEC-301 written and valid
- [x] Regression test: a lost run's final screen holds its hub's output (fails on dev)
- [x] mark_lost saves the hub's screen before recording the loss
- [x] Docs and changes entry true
- [ ] Independent review answered
- [ ] Gates green and PR merged on green

## Acceptance criteria

- A run whose host is killed while the server runs, after its turn settled (its `final-screen`
  saved) and more output reached its hub, ends `stopped(lost)` with a `final-screen` that shows
  that output.
- A loss recorded after another observer already recorded the run's end saves nothing.
- `a_screen_is_saved_only_from_a_hub_with_output` (existing) still passes: no blank overwrite.
- SPEC-229's reboot test still passes (its loss is recorded with an empty hub: the file stays).

## Test plan

- New integration test in `crates/yulius/tests/task_done.rs`:
  `a_run_lost_while_the_server_runs_saves_its_live_screen` (lost-connection path through
  `kill_run_host`). Run on dev's code first to show it fails on the stand-in, then with the fix.
- Existing: the `task_done::` file's tests and the `save_screen` unit test; the full gate.

## Decisions

- Save before the transition, not after: `finalize_attempt` does the same, and the terminal reads
  `final-screen` once no live attempt is left, so a save after the transition could be read stale.
  A losing observer's save is harmless: it writes the same hub's screen whole.
- No new ADR: this applies ADR-0177's "the run's end writes it again" to the lost end too.
