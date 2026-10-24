---
yulius-task: d70cfa55-33e7-4766-8500-30e1e3d2717d
status: done
---

# SPEC-301: a run lost while Yulius runs saves its live screen

Status: done
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
  reconciliation's server-lost branches. In reconciliation the runtime is new and normally has no
  attached run and no output, so the saved file stays there.
- Why the tests missed it: the SPEC-229 reboot test kills the processes while no server runs, so
  its loss is recorded by a new server with an empty hub; no test checked the screen of a run lost
  while the server was up.

## Requirements

- LRS-001: When `mark_lost` records a loss, the task's runtime exists and its attached run is this
  attempt, it saves the hub's screen to the attempt's `final-screen` with `save_screen` before the
  attempt becomes `Lost` (as `finalize_attempt` does before its transition), so no reader sees the
  run ended with the old file. A run lost before Yulius attached to it saves nothing: until the
  attach resets the hub, the hub shows the run before.
- LRS-002: A hub without output never overwrites the saved screen (`save_screen`'s existing rule);
  a deleted task's runtime is not brought back to save one.

## Plan

- [x] Spec SPEC-301 written and valid
- [x] Regression test: a lost run's final screen holds its hub's output (fails on dev)
- [x] mark_lost saves the hub's screen before recording the loss
- [x] Docs and changes entry true
- [x] Independent review answered
- [x] Attach-time loss saves no earlier run's screen (review rounds 1-2)
- [x] Gates green on the delivered head

## Acceptance criteria

- A run whose host is killed while the server runs, after its turn settled (its `final-screen`
  saved) and more output reached its hub, ends `stopped(lost)` with a `final-screen` that shows
  that output.
- A continued task's new run lost before it attached (its stop's escalation, the attach held)
  has no `final-screen`: the previous run's screen, still in the hub, is not saved as its own.
- `a_screen_is_saved_only_from_a_hub_with_output` (existing) still passes: no blank overwrite.
- SPEC-229's reboot test still passes (its loss is recorded with an empty hub: the file stays).

## Test plan

- New integration test in `crates/yulius/tests/task_done.rs`:
  `a_run_lost_while_the_server_runs_saves_its_live_screen` (lost-connection path through
  `kill_run_host`). Run on dev's code first to show it fails, then with the fix.
- New integration test `a_run_lost_before_it_attached_saves_no_screen_of_the_run_before`
  (`GatedHost` holds the continued run's attach, the stop's escalation loses the run): fails on
  the first fix (the previous run's `GOAL SET` screen saved), passes with the attached-run check.
- New integration test `a_run_lost_as_it_attaches_saves_no_screen_of_the_run_before`: the pause
  point `RunAttached` (the run just stored as attached, before a pending stop asks it to end)
  waits until the stop's escalation lost the run; fails while the hub is reset after that point.
- Existing: the `task_done::` file's tests and the `save_screen` unit test; the full gate.

## Decisions

- Save before the transition, not after: `finalize_attempt` does the same, and the terminal reads
  `final-screen` once no live attempt is left, so a save after the transition could be read stale.
  A losing observer's save is harmless: it writes the same hub's screen whole.
- Which run the hub shows is told by the runtime's attached run (`rt.run`), which `attach_run` sets
  and `finalize_attempt` and `mark_lost` clear: a loss after `finalize_attempt` cleared it saves
  nothing, so no check of the attempt's state is needed (an earlier draft had one; review).
- For that to hold, `attach_run` resets the hub to the run's snapshot and stores the attached run
  together, under the run's lock (review round 2: it stored the run first and reset the hub only
  after a stop's shutdown request, up to 2 s later). Viewer input waits on that lock, so input is
  still taken as soon as the screen shows. Rejected: recording in the hub which attempt it shows
  (a second source of the same fact).
- No new ADR: this applies ADR-0177's "the run's end writes it again" to the lost end too.

## Evidence

- Regression test first: `cargo nextest run -p yulius -E 'binary_id(yulius::main) &
  test(/^task_done::a_run_lost_while_the_server_runs_saves_its_live_screen$/)'` at a6af50c0 (dev's
  `mark_lost`): FAILED, the saved screen ended at `ECHO: one more`, without the output written
  after the settled turn. Passes from a11a62ba.
- Review round 1 (fresh read-only session, head e42f7fcf): APPROVE with 2 minor and 2 nits, all
  own code. Minor 1 (a run lost before it attached saved the run before's screen): reproduced by
  `a_run_lost_before_it_attached_saves_no_screen_of_the_run_before` at 4687ced4 (the round-1 fix)
  (FAILED, `GOAL SET` of the first run in the second run's `final-screen`), fixed in a28e1e96 by
  saving only for the runtime's attached run. Minor 2 (the attempt-state check had no test): the
  check was removed as redundant, see Decisions. Nit 3: Context reworded ("normally"). Nit 4: the
  test says why it feeds the hub directly.
- Review round 2 (fresh session, head 5d126b90): APPROVE with 1 minor and 1 nit, own code. Minor
  (a loss between storing the attached run and the hub's reset saved the run before's screen):
  reproduced by `a_run_lost_as_it_attaches_saves_no_screen_of_the_run_before` at b0d392f3 (FAILED,
  `GOAL SET` saved), fixed by resetting the hub and storing the run together under its lock. Nit
  (the Plan said merged before the merge): the last step is now "Gates green on the delivered
  head".
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^task_done::/) |
  test(/^supervision::/) | test(/^terminal/))'` after the round-2 fix: 75 passed.
- `scripts/dev ci` at 3de32acf: exit 0, 2602 tests passed (1 slow), 16 skipped; repository
  contracts passed.
- `scripts/dev security`: exit 2, not run: Vidoc answered HTTP 429 "This API key has used its
  50,000,000 tokens" (2 files not scanned). Unrun, not passed (ADR-0133, ADR-0157); CI's
  `security` job scans the pull request head.
- Not tested: the `arm_escalation` loss of an attached run and reconciliation's server-lost
  branches have no screen test of their own; they reach the same `mark_lost` code, and SPEC-229's
  reboot test still passes (reconciliation keeps the saved screen).
