---
yulius-task: 9df61d43-e974-4e41-90dd-b18f9fda32c1
status: in-progress
---

# SPEC-377: the skip answer is true when a chosen step was removed or skipped meanwhile

Status: in-progress
Date: 2026-11-01
Owner: Yulius task #359
Scope authorization: task #359, Fixes #944 (a nit left by the review of #942, SPEC-341).

## Goal

When a person's "skip after" on a held prompt finds none of the chosen steps still open, its
answer says what can have happened to them: finished, skipped or removed meanwhile, instead of
"(already finished)", which is false for the last two.

## Context

- Symptom: `held_skip_after` (`crates/yulius-app/src/intake.rs`) answers "No open step was left to
  skip (already finished); #N runs after this task." whenever it skipped nothing.
- Root cause: since SPEC-341 LSS-002 the skip re-reads the steps under the task's steps lock and
  skips a chosen step only while it is still listed and unresolved. A step can stop being open in
  three ways while the skip waits for the lock: the agent or the spec sync finished it (`done`),
  someone else or the spec sync skipped it (`skipped`), or the spec sync or `steps_set` replaced
  the list without it (`replace_steps` drops its id). All three fall through to the same answer,
  written for the first case only. The one test of this path,
  `intake::a_step_finished_while_the_skip_waits_stays_done`, covers only `done`.
- What else the cause could break: nothing. The steps are left as they are in every case (a
  removed step stays removed, a resolved one keeps its status); only the answer's text was wrong.
  No other code or the web UI reads that text (`grep "No open step"`).

## Requirements

- **SAZ-001** When the skip finds none of the chosen steps still open, the answer is
  "No open step was left to skip (finished, skipped or removed meanwhile); #N runs after this
  task." It is true for each of the three cases, which the skip cannot reliably tell apart by
  author: `skipped` also comes from the spec sync.
- **SAZ-002** A chosen step removed from the list while the skip waits for the steps lock is not
  recreated, the chained task is still created, and the answer is SAZ-001's (status 200).

## Plan

- [x] Spec for the skip answer's zero case
- [ ] Regression test for a step removed while the skip waits
- [ ] Skip answer covers finished, skipped and removed steps
- [ ] Gates, review and delivery

## Acceptance criteria

- The zero-skipped answer is true when the chosen step was finished, removed or skipped meanwhile
  (SAZ-001).
- A test covers a step removed while the skip waits for the steps lock (SAZ-002), and failed
  before the fix.

## Test plan

- New `intake::a_step_removed_while_the_skip_waits_stays_removed`: holds the task's steps lock,
  waits for the chained task (the skip now waits for the lock), replaces the steps without `st-2`,
  releases the lock; asserts 200, the SAZ-001 answer (not "already finished") and `st-2` gone.
- `intake::a_step_finished_while_the_skip_waits_stays_done` asserts the full SAZ-001 text.
- Run `intake::` with nextest, then `scripts/dev ci`.

## Decisions

- **One wording for all three cases, not a reason per step.** Telling "removed" from "resolved"
  is possible, but "skipped by another person" cannot be told from "skipped by the spec sync", and
  with several chosen steps a per-step reason would make a long answer for a rare race. The issue
  suggests this wording; it is reversible. No ADR: no lasting design choice.
- The partial case ("1 step skipped" while another chosen step was resolved) keeps its answer: it
  is true, and the step list shows the rest.
