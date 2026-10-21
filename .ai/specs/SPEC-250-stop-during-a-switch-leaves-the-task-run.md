---
yulius-task: b15ccbdb-8401-42e1-b159-447cb7ae64d1
status: in-progress
---

# SPEC-250: A stop always takes effect, even while a switch completes

Status: in progress
Date: 2026-10-21
Owner: Yulius task #236
Scope authorization: task #236, started by task #235 (work through open issues). Fixes #527. No
new GitHub issue is created. Refines SPEC-065 (END-001) and SPEC-140 (STP-001, STP-002).

## Goal

A stop of a live task always ends it `Stopped`: when the stop's status change loses to a concurrent
one (a switch completing, a start), or the run ends before the status change lands, the task does
not stay `Running` or `Stopping` with no run.

## Context

`task_done::a_switch_waits_for_a_background_agent_launched_in_the_turn` fails intermittently in
Yulius CI (#527: PR #522, #590, #646, merge train #672 + #670, task #166's local gate) with
`timed out waiting for the stop after 60s: … running (runs: … Killed Some("switched"), … Killed
Some("cancelled"))`. Its last step waits for the switch's new run, then stops the task.

Root cause, in `stop_task_now` (`crates/yulius-app/src/supervisor.rs`):

1. The stop reads the task: `Switching`, the switch's new run exists but the launch has not
   applied `SwitchCompleted` yet.
2. It sets the run's end intent to `Cancel` and calls
   `transition_task(Some(Switching) → Stopping)`.
3. The launch applies `SwitchCompleted` (`Switching → Running`) in between: the compare-and-set
   returns `false`, which the stop ignores.
4. The stop still ends the run (`request_shutdown`, or `attach_run`, which sees the `Cancel`
   intent). The run's end handler (`finalize_attempt`) applies `Reaped`, which only a `Stopping`
   task takes, so it changes nothing.
5. The task stays `Running` with no live run; nothing ends it.

A second window of the same race: the run ends by itself (or by an earlier request) after the stop
set its `Cancel` intent but before its status change. The end handler then finds the task still
`Running`, `Reaped` does not apply, and the stop's change then moves the task to `Stopping` with no
run left: nothing escalates (no live run) and nothing records an end. Retrying the stop (point 1)
widens this window, since the intent of a lost round stays set.

The existing tests missed it because they stop a task whose status is stable: the window between
the stop's read and its compare-and-set is a few awaits wide.

## Requirements

- STW-001: when `stop_task_now`'s status change loses to a concurrent one, it reads the task again
  and stops it from the new status: a live status goes to `Stopping` and its run is ended; a status
  that waits (queued, chained, prepared, waiting for capacity with no run) goes to `Stopped`. The
  rounds are bounded ([`STOP_ROUNDS`]); a task still changing after them is refused with
  `Conflict` (try again).
- STW-002: a task that rests when a lost round reads it again was ended meanwhile (by this stop's
  intent, or by its run's own end): the stop succeeds. A stop of a task that already rests at its
  first read is refused as before (`Conflict`).
- STW-003: a run that ends with the `Cancel` intent while its task is still live (`Starting`,
  `Running`, `Switching`; the stop's status change not made yet) ends its task
  `Stopped(Cancelled)`, as it would from `Stopping`.
- STW-004: test-only pause points (ADR-0174) let a test change the world exactly between the
  stop's read and its status change; production builds do nothing there.

## Plan

- [ ] Spec SPEC-250 written and validated
- [ ] Regression test reproduces the lost stop (fails on dev)
- [ ] stop_task_now retries a lost compare-and-set from the re-read status
- [ ] Other ignored transition_task results reviewed and recorded
- [ ] Flaky switch test passes repeatedly under load
- [ ] Independent review answered, gates green, docs updated

## Acceptance criteria

- A deterministic test stops a task while its switch's new run is held at attach and completes the
  switch between the stop's decision and its compare-and-set: it fails on dev (the task stays
  `Running`) and passes with the fix (`Stopped(Cancelled)`, no live run).
- A deterministic test ends a running task's run between the stop's decision and its
  compare-and-set: it fails on dev (the task stays `Stopping` with no run) and passes with the fix.
- `task_done::a_switch_waits_for_a_background_agent_launched_in_the_turn` passes repeatedly under
  probe delays and CPU load.
- The other ignored `transition_task` results in `supervisor.rs` are reviewed; the review is
  recorded below.

## Test plan

- `task_done::a_stop_that_loses_to_a_completing_switch_still_ends_the_task`: `GatedHost` holds the
  switch's new run at attach; the stop pause point releases it and waits until the task is
  `Running`; the stop must end the task `Stopped(Cancelled)`.
- `task_done::a_run_that_ends_before_the_stops_change_still_ends_the_task`: the stop pause point
  terminates the run's host and waits for its end to be recorded; the stop must end the task
  `Stopped(Cancelled)` with no live run.
- The flaky test under probe delays at the stop's pause point and under CPU load, many runs.
- `scripts/dev ci` (the gate) and `scripts/dev security`.

## Decisions

- **Retry the stop from the re-read status, bounded**, rather than holding a lock across the
  stop and the launch's status change: the launch holds `supervisor.launch` across slow I/O (spawn,
  attach), so a stop that waited for it would wait for a whole launch (SPEC-140 STP-001 stops a
  launching run at once). A compare-and-set that lost means another writer changed the status;
  the status machine has no cycle a stop can keep losing to, so a few rounds suffice; `Conflict`
  after them keeps the bound explicit (AGENTS.md: every retry has a bound).
- **The end handler rests a live task whose run ended by a stop's intent** (STW-003) rather than
  the stop re-checking for a run that is gone after its change: the stop cannot see an end being
  recorded (the `finalizing` window), the end handler is where the end is known. The `Cancel`
  intent is set only by `stop_task_now`, so it always means a person's stop.
- **Pause points keyed by task**: `race::reached` takes the task id as its key path for the stop's
  points, so tests running in parallel do not take each other's hooks (ADR-0174's contract). No new
  ADR: this applies ADR-0174 and SPEC-065's stop contract; it decides nothing lasting.
