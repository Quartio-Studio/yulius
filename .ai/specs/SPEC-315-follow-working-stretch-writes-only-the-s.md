---
yulius-task: caec492e-6f52-4a93-ae7d-5224e86a15a2
status: in-progress
---

# SPEC-315: The idle task's stretch write applies only to the state it was decided on

Status: in-progress
Date: 2026-10-25
Owner: task #301
Scope authorization: task #301, issue #895 (found by the independent review of task #295,
SPEC-309).

## Goal

Publishing an idle task ends or starts its working stretch (SPEC-087 TIM-002) only while the task
is still in the state that decision was made on. A turn that starts in between keeps its stretch.

## Context

- Symptom (issue #895): a publish racing a turn's start (the shell probe's publish while a typed
  prompt's hook runs) can leave a Working task with `working_since = NULL`; the store then starts a
  new stretch at the task's next activity change, and the sidebar's "working …" starts over.
- Code path: `App::publish_task` reads the task, and `App::follow_working_stretch`
  (`crates/yulius-app/src/tasks.rs`) decides from its status and activity (`idle_stretch`), then
  reads `task_times`, then calls `set_working_since(&task.id, next)`. The store runs
  `UPDATE tasks SET working_since = ?1 WHERE id = ?2` with no condition
  (`crates/yulius-store/src/tasks.rs`). When the prompt hook makes the task Working
  (`set_activity`/`set_activity_from`, which keep or start `working_since`) between the reads and
  the write, the write still ends the stretch of a task that is now Working.
- Why the tests missed it: only `SqliteStore` implements the store port, so no test can stop
  `follow_working_stretch` between its read and its write; the store tests cover
  `set_working_since` as an unconditional write, and the integration tests see the race only by
  chance.
- Pre-existing on `dev` before SPEC-309; SPEC-309 left this write as it was.
- What else the cause could break: the `(true, None)` branch (an idle task with background work
  starts a stretch) races the same way: a stretch that `continue_working_stretch` restored in
  between was overwritten with a new start. `continue_working_stretch` and `keep_working_stretch`
  write unconditionally on purpose (SPEC-309) and stay as they are.

## Requirements

- FWS-001: the stretch write of `follow_working_stretch` applies only while the task is running
  with the activity and state time it was decided on and still has the working stretch that was
  read; otherwise it changes nothing.
- FWS-002: the store reports whether the conditional write applied.

## Plan

- [x] Root cause and spec of the unconditional stretch write
- [x] Regression test: a stale stretch write leaves a Working task's stretch (failing)
- [x] The stretch write applies only to the state it was decided on
- [x] Docs, changes and review answered
- [ ] Gates green and delivered on dev

## Acceptance criteria

- A task read idle at state time T, then made Working by `set_activity_from`, keeps its
  `working_since` when the stretch write is called with the stale `(idle, T)` and the read times;
  the store answers that the write did not apply (FWS-001, FWS-002).
- A write whose read stretch is no longer the task's (another writer changed `working_since` in
  the same state) does not apply.
- With the state it read unchanged, the write ends and starts the stretch as before (TIM-002).

## Test plan

- Store (`crates/yulius-store/tests/store.rs`,
  `a_stretch_write_applies_only_to_the_state_it_read`): the race played in sequence on
  `SqliteStore`: read an idle task's times, make it Working with `set_activity_from`, call the
  conditional write with the stale state, assert it did not apply and `working_since` is kept; a
  stale stretch with the current state does not apply either; the current state applies (end
  and start). Fails before the fix: committed first against a conditional write that ignored its
  condition, as `set_working_since` does.
- App unit test (`tasks::tests::an_ended_stretch_is_forgotten_only_once_recorded`): the stretch the
  start continues stays remembered until it is recorded, and only that one is forgotten. Fails
  against the take of the remembered stretch (committed first with that behaviour).
- App: the existing stretch tests (`tasks::tests::*stretch*` in `yulius-app`,
  `task_done::*working_stretch` in `yulius`) keep passing with the guarded write.

## Decisions

- **A new conditional store write, `set_working_since_from`, beside `set_working_since`.** Modelled
  on `set_activity_from` (SPEC-213). `continue_working_stretch` and `keep_working_stretch` must
  write whatever a publish did in between (SPEC-309), so the unconditional write stays for them.
- **The condition is everything the decision read**: status running, the activity of the task
  record, and both times (`state_at` and `working_since`). The activity matters because the task and
  its times are read separately; `working_since` makes the `(true, None)` start leave a stretch
  `continue_working_stretch` restored in between, and the end leave a stretch that changed.
- **The remembered ended stretch is still recorded before the write**, as SPEC-309 needs (the wake
  must find it once `working_since` is gone). When the write does not apply, the remembered stretch
  carries the stale state time, so `continued_stretch` never continues from it (WSW-002).
- **A stretch the start continues is forgotten only once that start is recorded**
  (`TaskRuntime::ended_stretch_from`, `TaskRuntime::forget_ended_stretch`). Before, the
  `(true, None)` branch took it before an unconditional write; with the write now refusable, a take
  would lose it when the task's own wake-up made the task Working in between, and that turn would
  start a new stretch. A refused write leaves it for `continue_working_stretch`; a person's prompt
  still cannot use it (its state time changed).
- **No app-level race test.** The race needs a store that pauses between the read and the write;
  the store port has one implementation, and a timing-based test would be flaky. The store test
  proves the guard, the call site passes the state it read.
- No ADR: this corrects TIM-002's implementation; it takes no new lasting direction.

## Review

Independent review (fresh session, `om-code-review`), round 1 at `e04073a3`: approve, with:

- Own, minor: in the `(true, None)` branch the remembered stretch (SPEC-309) was taken before the
  now-refusable write, so a refused write lost it and the task's own wake-up started a new stretch.
  Fixed test-first: it is forgotten only once its start is recorded
  (`an_ended_stretch_is_forgotten_only_once_recorded`, failing at `337b22f3` with a take-style stub,
  passing with the fix).
- Own, nit: a refused write was silent. Fixed: logged at debug.
- Own, nit: Plan, Review and Evidence sections. Done.
