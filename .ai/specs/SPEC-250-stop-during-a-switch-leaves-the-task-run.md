---
yulius-task: b15ccbdb-8401-42e1-b159-447cb7ae64d1
status: done
---

# SPEC-250: A stop always takes effect, even while a switch completes

Status: done
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
  rounds are bounded (`STOP_ROUNDS`); a task still changing after them is refused with
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
- STW-006: a switch never takes over the end of a run that a stop or a close owns, and starts no
  new run once overtaken:
  - it sets its intent for the old run only when the slot holds no stop's or close's intent, and
    goes on only while the task is still `Switching` (or paused for capacity meanwhile, which the
    launch takes as before);
  - an intent replaced while it waits (a stop, a close, or the run's loss dropping it) ends the
    switch;
  - it launches only from the status it goes on from, checked under the launch lock (a stopped
    task is neither started again nor queued);
  - a task its stop left `Stopping` with no run (the stop came once the old run had ended) rests
    `Stopped(Cancelled)` when the switch gives up, as a second stop would rest it;
  - an overtaken switch records its outcome as `cancelled`, tells the viewers why ("the task was
    stopped", "the task was closed", "the session ended") and returns `Conflict`; the stop, close or
    loss that replaces the switch's intent sends that reason through the switch's channel
    (`SwitchEnd`), so it does not depend on what the run's end has recorded yet;
  - the end of a run closed during a switch closes the task from `Switching`.
- STW-007: a stop that finds the task resting after a lost round still ends a run that is live; a
  stop that gives up after its rounds drops its intent, so no run ends later as if stopped.
- STW-004: test-only pause points (ADR-0174) let a test change the world exactly between the
  stop's read and its status change (`StopDecided`), a run end's (`EndDecided`), a switch's status
  change and its intent (`SwitchDecided`), and a close's read and its intent (`CloseDecided`);
  production builds do nothing there.

## Plan

- [x] Spec SPEC-250 written and validated
- [x] Regression test reproduces the lost stop (fails on dev)
- [x] stop_task_now retries a lost compare-and-set from the re-read status
- [x] Other ignored transition_task results reviewed and recorded
- [x] Flaky switch test passes repeatedly under load
- [x] Docs updated: architecture, changes, spec decisions
- [x] Gate green on the merged head
- [x] Review round 1 findings fixed test-first
- [x] Independent review approves the head

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
  the old run, so the switch waits (the test waits until the switch's shutdown request was
  dropped, so the switch's intent is set); the stop pause point waits until the switch gave up (or
  started a run); the task must end `Stopped(Cancelled)` with one run, the switch refused.
- `task_done::a_stop_before_a_switch_sets_its_intent_starts_no_new_run`: the switch pause point
  stops the task; the switch must give up, the task end `Stopped(Cancelled)` with one run.
- `task_done::a_close_that_overtakes_a_switch_closes_the_task`: the close pause point starts a
  switch and waits until it asked the old run to end; the switch must give up ("closed") and the
  task end `Closed` with one run.
- `task_done::a_switch_whose_old_run_is_lost_starts_no_new_run`: the old run's host is killed
  while the switch waits; the switch must give up ("the session ended") and the task rest
  `Stopped(Lost)` with one run.
- `task_done::a_stop_that_ended_no_run_does_not_end_the_next_one` (SPEC-140, changed): the stop
  that comes once the switch's old run ended, while another task's launch holds the lock, now
  rests the task when the switch gives up (`Stopped(Cancelled)`, no second stop).
- `task_done::a_switch_paused_for_capacity_goes_on`: the switch pause point moves the task to
  `WaitingCapacity`; the switch must go on and launch (two runs).
- `task_done::a_switch_whose_paused_task_another_start_runs_is_completed`: the task is paused at
  the switch's decision and run (`Running`) before the switch's launch takes the lock; the switch
  must count as made, not overtaken.
- `task_done::a_stop_while_a_runs_loss_is_recorded_still_ends_the_task`: the run's host is killed
  and the end pause point stops the task during `mark_lost`; the task must rest `Stopped(Lost)`.
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
  fixed in its own commit rather than left to fail this pull request's checks. `dev` fixed it the
  same way meanwhile; the merge keeps `dev`'s version.
- **Pause points keyed by task**: `race::reached` takes the task id as its key path for the stop's
  points, so tests running in parallel do not take each other's hooks (ADR-0174's contract). No new
  ADR: this applies ADR-0174 and SPEC-065's stop contract; it decides nothing lasting.

## Evidence

- **Reproduction on dev:** `a_stop_that_loses_to_a_completing_switch_still_ends_the_task` failed
  on the unfixed code with `timed out waiting for the stop: running (runs: Killed Some("switched"),
  Killed Some("cancelled"))`, the signature #527 reports from CI. In its final form (attach held
  too) it fails with `left: Running, right: Stopping`.
- **Each regression test failed without its fix and passes with it**, run with
  `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^task_done::…/)'`:

  | Test | Failure without the fix |
  |---|---|
  | `a_run_that_ends_before_the_stops_change_still_ends_the_task` | `stopping (runs: Killed Some("cancelled"))` |
  | `a_stop_while_a_runs_end_is_recorded_still_ends_the_task` | `stopping (runs: Exited Some("exited"))` |
  | `a_stop_while_a_runs_loss_is_recorded_still_ends_the_task` (single-shot `mark_lost`) | `stopping (runs: Lost Some("lost"))` |
  | `a_stop_while_a_switch_ends_its_old_run_starts_no_new_run` | `stopped_switch_failed (runs: Live None)` |
  | `a_stop_before_a_switch_sets_its_intent_starts_no_new_run` | fails |
  | `a_close_that_overtakes_a_switch_closes_the_task` | fails |
  | `a_switch_whose_old_run_is_lost_starts_no_new_run` | fails |
  | `a_stop_that_ended_no_run_does_not_end_the_next_one` (changed) | `stopping (runs: Killed Some("switched"))` |
  | `a_switch_paused_for_capacity_goes_on` | `Err("the session ended during the switch")` |
  | `a_switch_whose_paused_task_another_start_runs_is_completed` | `Err("the session ended during the switch")` |
  | `a_close_that_overtakes_a_switch_closes_the_task` (reason held) | `Err("the session ended during the switch")` |

- **The originally flaky test under load:** temporary probe delays (0-400 ms before the launch reads
  the task for `SwitchCompleted`, 0-300 ms after the stop's read; removed, `grep PROBE527` finds
  none) at host load average 138-160 on 12 CPUs. `a_switch_waits_for_a_background_agent_launched_in_the_turn`,
  run 3 at once:

  | Code | Runs | Failures |
  |---|---|---|
  | dev's `supervisor.rs` | 12 | 3, all `timed out waiting for the stop after 60s: … running (runs: … Killed Some(")…` |
  | this change | 24 | 0 (20-22 s each) |

- **Gate:** `scripts/dev ci` exit 0 on the merged head `b9cf86ba` (2383/2383 tests, fmt, clippy for
  Linux and macOS, doctests, deny, shear, doc), and again on the final head (see the pull request).
  After the later fixes, the stop, switch, close, supervision, rotation, limit and context tests
  pass (145/145).
- **Security:** `scripts/dev security` exit 2 (Vidoc HTTP 429: the instance key's token quota is
  used up). The changed code files (4 against the merge base at the end) were not scanned, so the
  scan is unrun, not passed; a person renews the key.
- **Independent review in fresh sessions:**

  | Round | Head | Verdict | What happened |
  |---|---|---|---|
  | 1 | `cdc8ec6e` / `05d9318e` | request changes | A `cargo doc` blocker and 3 majors (stop before the switch's intent, close overtaking a switch, a flaky test), fixed test-first |
  | 2 | `5741dada` | request changes | 1 major (stop after the old run ended), fixed test-first |
  | 3 | `b9cf86ba` | approve | 1 minor and 3 nits in this change's code, fixed (the minor test-first) |
  | 4 (delta) | `fe398143` | request changes | 1 major: the close-overtakes-a-switch test was flaky (the switch read its reason before the run's end settled). Made deterministic with the `SwitchOvertaken` pause point (`Err("the session ended during the switch")` before the fix), fixed with `SwitchEnd`; 15 of 15 runs pass |

- **Outside this change:** recorded on #527, since this task files no issues.
  - A stale `Switch` intent after a switch timeout: pre-existing, found by reading, not reproduced.
  - `projects::a_scheduled_start_without_claude_waits_and_says_why` was flaky; `dev` fixed it meanwhile.
  - `task_done::a_typed_compaction_leaves_a_done_task_done` timed out once at load 160 and passed 8 of 8 reruns; it touches no stop, switch or run-end code.
