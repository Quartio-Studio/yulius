---
yulius-task: 9df61d43-e974-4e41-90dd-b18f9fda32c1
status: done
---

# SPEC-377: the skip answer is true when a chosen step was removed or skipped meanwhile

Status: done
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
- A fourth way to skip nothing, found by the review: the card's "Skip the selected steps…" button
  posts `skip_step_ids: []` when no step is checked (it is the only way a `drops_scope` card runs
  the new direction after the task), and the answer then claimed a chosen step had stopped being
  open (before this change: "already finished").

## Requirements

- **SAZ-001** When the skip finds none of the chosen steps still open, the answer is
  "No open step was left to skip (finished, skipped or removed meanwhile); #N runs after this
  task." It is true for each of the three cases, which the skip cannot reliably tell apart by
  author: `skipped` also comes from the spec sync.
- **SAZ-002** A chosen step removed from the list while the skip waits for the steps lock is not
  recreated, the chained task is still created, and the answer is SAZ-001's (status 200).
- **SAZ-003** With no step chosen, the skip still chains the new direction, skips nothing and
  answers "No step was chosen to skip; #N runs after this task."

## Plan

- [x] Spec for the skip answer's zero case
- [x] Regression test for a step removed while the skip waits
- [x] Skip answer covers finished, skipped and removed steps
- [x] Gates, review and delivery

## Acceptance criteria

- The zero-skipped answer is true when the chosen step was finished, removed or skipped meanwhile
  (SAZ-001).
- A test covers a step removed while the skip waits for the steps lock (SAZ-002), and failed
  before the fix.
- An empty selection says no step was chosen (SAZ-003), with a test that failed before.

## Test plan

- New `intake::a_step_removed_while_the_skip_waits_stays_removed`: holds the task's steps lock,
  waits for the chained task (the skip now waits for the lock), replaces the steps without `st-2`,
  releases the lock; asserts 200, the chained task, the full SAZ-001 answer and `st-2` gone.
- `intake::a_step_finished_while_the_skip_waits_stays_done` asserts the full SAZ-001 text.
- New `intake::a_skip_with_no_chosen_step_says_so`: `skip_step_ids: []` answers SAZ-003, chains
  the task and leaves every step pending.
- Run `intake::` with nextest, then `scripts/dev ci`.

## Decisions

- **One wording for all three cases, not a reason per step.** Telling "removed" from "resolved"
  is possible, but "skipped by another person" cannot be told from "skipped by the spec sync", and
  with several chosen steps a per-step reason would make a long answer for a rare race. The issue
  suggests this wording; it is reversible. No ADR: no lasting design choice.
- **An empty selection stays allowed** (SAZ-003) rather than answering 422: on a `drops_scope`
  card the skip button is the only way to run the new direction after the task, and skipping
  nothing is a fair choice. Only its answer changes.
- The "skipped by someone else" case has no test of its own: it takes the same `is_resolved`
  branch as the `done` test.
- The partial case ("1 step skipped" while another chosen step was resolved) keeps its answer: it
  is true, and the step list shows the rest.

## Evidence

- **Red.** `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^intake::a_step_/)'`
  before the fix: `a_step_removed_while_the_skip_waits_stays_removed` failed with the note "No
  open step was left to skip (already finished); #2 runs after this task." (bc805f69). Before
  SAZ-003, `a_skip_with_no_chosen_step_says_so` failed with "No open step was left to skip
  (finished, skipped or removed meanwhile); #2 runs after this task." (3dbb1cdc).
- **Green.** `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^intake::/)'`: 17
  tests, all passed.
- **Gate.** `scripts/dev ci` exited 0 at `c2238bee` (2950 passed, 16 skipped) and at `e41d8c92`
  (2951 passed, 16 skipped): fmt, lint, macOS compile, deps, nextest, doctests and docs. Later
  commits change only `docs/api.md` and this spec.
- **Security.** `yulius security-scan` exited 0: "Security scan disabled" (Vidoc is off for this
  project in Yulius). Nothing was scanned; this is not a pass.
- **Review.** Fresh-session `om-code-review`: round 1 approved `c2238bee` with one pre-existing
  minor (the empty selection, fixed test-first as SAZ-003), the unfinished spec, an overclaiming
  test comment and loose assertions (all fixed), and no test for "skipped by someone else"
  (answered in Decisions). Round 2 approved `e41d8c92`; its nits: the `docs/api.md` clause was
  added, and the pre-existing `starts_with("1 step skipped")` check in the happy-path test is left
  as it is (true, outside this change).
- **Not tested.** No browser run: the web card only shows `note` in a toast, unchanged.
