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

Two more windows of the same race were found while reviewing the other ignored results:

- The end of a run (`mark_lost`, `finalize_attempt`) reads the task, then makes its status change
  and ignores the result. A stop landing in between moves the task to `Stopping`; the end's change
  then fails and the task stays `Stopping` with no run.
- A stop that lands while a switch waits for its old run to end replaces the switch's intent
  (`EndIntent::Switch`), which drops the switch's channel: the switch wakes at once and launches a
  new run while the old one is still ending. On dev that launch failed and rested the task as
  `stopped_switch_failed` with the old run still live; with the stop's retry alone, the task could
  rest with a new run that nobody ends.

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
- STW-005: the end of a run (lost or exited, with or without an intent) whose status change loses
  to a concurrent one makes it again from the status read anew (bounded like a stop), so a stop
  landing meanwhile still ends `Stopped`.
- STW-006: a switch whose intent is replaced while it waits for its old run to end (a stop or a
  close took over that end, or the run's loss dropped it) starts no new run: its outcome is
  recorded as `cancelled` and it returns `Conflict`. A stop that finds the task resting after a lost
  round still ends a run that is live.
- STW-004: test-only pause points (ADR-0174) let a test change the world exactly between the
  stop's read and its status change (`StopDecided`) and between a run end's read and its change
  (`EndDecided`); production builds do nothing there.

## Plan

- [x] Spec SPEC-250 written and validated
- [x] Regression test reproduces the lost stop (fails on dev)
- [x] stop_task_now retries a lost compare-and-set from the re-read status
- [x] Other ignored transition_task results reviewed and recorded
- [x] Flaky switch test passes repeatedly under load
- [x] Docs updated: architecture, changes, spec decisions
- [ ] Gate green on the merged head
- [ ] Independent review answered

## Acceptance criteria

- A deterministic test stops a task while its switch's new run is held at attach and completes the
  switch between the stop's decision and its compare-and-set: it fails on dev (the task stays
  `Running`) and passes with the fix (`Stopped(Cancelled)`, no live run).
- A deterministic test ends a running task's run between the stop's decision and its
  compare-and-set: it fails on dev (the task stays `Stopping` with no run) and passes with the fix.
- `task_done::a_switch_waits_for_a_background_agent_launched_in_the_turn` passes repeatedly under
  probe delays and CPU load.
- A deterministic test lands a stop between a run end's read and its change: it fails on dev
  (`Stopping`, no run) and passes with the fix.
- A deterministic test stops a task while its switch waits for the old run to end: it fails on dev
  (`stopped_switch_failed`, the old run live) and passes with the fix (`Stopped(Cancelled)`, one
  run, the switch refused).
- The other ignored `transition_task` results in `supervisor.rs` are reviewed; the review is
  recorded under Decisions.

## Test plan

- `task_done::a_stop_that_loses_to_a_completing_switch_still_ends_the_task`: `GatedHost` holds the
  switch's new run at its spawn (`Switching`) and at its attach; the stop pause point releases the
  spawn and waits until the attach is held (`Running`); the stop must leave the task `Stopping`, and
  once the attach goes on, `Stopped(Cancelled)` with no live run.
- `task_done::a_run_that_ends_before_the_stops_change_still_ends_the_task`: the stop pause point
  makes the agent exit (`/exit`) and waits for its end to be recorded; the stop must end the task
  `Stopped(Cancelled)` with no live run.
- `task_done::a_stop_while_a_runs_end_is_recorded_still_ends_the_task`: the end pause point stops
  the task; the task must end `Stopped(Cancelled)`.
- `task_done::a_stop_while_a_switch_ends_its_old_run_starts_no_new_run`: the host refuses to end
  the old run, so the switch waits; the stop pause point waits until the switch gave up (or started
  a run); the task must end `Stopped(Cancelled)` with one run, the switch refused.
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
- **A switch overtaken by a stop gives up** (STW-006) rather than continuing when its intent
  is replaced: the old run's end belongs to the stop (or close) then, and a new run started beside
  it would outlive the stop. A switch whose old run was lost gives up too (the task rests
  `Stopped(lost)`; Continue resumes it), the conservative choice: no run starts that no request
  asked for after the loss.
- **Review of the other ignored `transition_task` results** (`supervisor.rs`, dev at 4e796d87):
  - fixed here: `stop_task_now`'s two status changes (STW-001), `mark_lost` and the three
    `finalize_attempt` branches (STW-005, through `settle_ended`);
  - a start that queues at the session limit (`ContinueQueued`) and a failed start or switch
    (`Failed`, `Stopped(SwitchFailed)`, each guarded by a re-read): a lost change there lost to a
    stop or close that owns the task from then on, and the start has no run to strand. A stop that
    wins against a failed launch leaves `Stopping` with no run, which the next stop rests
    (SPEC-140 STP-002, unchanged);
  - the launch's `Spawned` / `SwitchCompleted`: it loses only to a stop, close or loss, which ends
    the run once attached (SPEC-140 STP-001);
  - the boot recovery (`reconcile`: a reattached run back to `Running`; a live task without a run
    to its loss). It runs in the background once the listeners are bound, so a stop can race it.
    A reattach that loses to a stop leaves `Stopping` with its run live, which the stop ends. A
    loss that loses to a stop leaves `Stopping` with no run, which the next stop rests. Both stay
    as they are: a window of one store round trip, once per restart, already recoverable.
- **A flaky test met on the way is fixed here, test-only**: the gate failed once in
  `projects::a_scheduled_start_without_claude_waits_and_says_why` (SPEC-246, #760, on dev before
  this change; 4 of 30 runs failed on this branch's base code path, which this change does not
  touch). The pause moves the task to `waiting_capacity` before it records the pause and sends the
  notice, and the test read the notices at once. It now waits for the notice (bounded, 20 s): 40 of
  40 passed. This task may not file issues, and a merge needs green CI without retries, so it is
  fixed in its own commit rather than left to fail this pull request's checks.
- **Pause points keyed by task**: `race::reached` takes the task id as its key path for the stop's
  points, so tests running in parallel do not take each other's hooks (ADR-0174's contract). No new
  ADR: this applies ADR-0174 and SPEC-065's stop contract; it decides nothing lasting.
