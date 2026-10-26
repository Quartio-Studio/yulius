---
yulius-task: 9af62d42-2393-4696-9155-3a5289c48ec0
status: done
---

# SPEC-319: The update's 30-minute cancel message counts every task still in a turn

Status: done
Date: 2026-10-26
Owner: Yulius task #304
Scope authorization: task #304 (`/goal`), fixes #533. Amends the cancel message of
[SPEC-160](SPEC-160-tasks-should-be-paused-imidietly-not-wai.md) (OPS-011's 30-minute bound);
no new decision.

## Goal

When an update's pause reaches its 30-minute bound, the cancel message "N task(s) were still in a
turn after 30 minutes; nothing was installed." gives the number of tasks that kept the update from
installing, however many there are.

## Context

- **Symptom.** With more than 20 tasks still in a turn at the bound, the message says 20. A run
  still being frozen at the bound is not counted at all.
- **Root cause.** `UpdateRun` draining (`crates/yulius-app/src/updates.rs`) takes N from
  `progress.waiting.len()`. `waiting` is the list shown on the instance page: `pause_progress`
  stops filling it at `MAX_WAITING_SHOWN` (20) and, since SPEC-160 OPS-014, leaves out runs that
  are still being frozen (`FreezeState::Freezing`). Readiness, in the same branch, uses
  `paused >= total`; the message never used the same count.
- **Why tests missed it.** `self_update.rs` `pausing_is_bounded_and_updates_can_be_cancelled` seeds
  two tasks (one in a turn) and checks only the text, not the number.
- **Reach.** Only the message: readiness, the install and the hold do not read `waiting`'s length.

## Requirements

| ID | Requirement |
| --- | --- |
| R1 | The 30-minute cancel message counts `total - paused` (saturating), the same counts readiness uses, not the shown list. |
| R2 | The instance page's "Still in a turn" list keeps its cap of 20 and keeps leaving out runs being frozen (OPS-014 unchanged). |

## Plan

- [x] Spec SPEC-319 written and validated
- [x] Failing regression test: 21 tasks in a turn at the bound
- [x] Cancel message counts total - paused
- [x] Gates green, docs/changes.md entry, spec evidence
- [x] Review round 1 findings answered
- [x] Review round 2 approves the final head

## Acceptance criteria

- With 21 tasks in a turn (and one idle) at the bound, the run is cancelled with "21 task(s) were
  still in a turn after 30 minutes; nothing was installed." while the page lists 20 of them.
- A run still being frozen at the bound (not confirmed, within its 5 passes) is counted.
- The existing bounded-pause test asserts the exact count ("1 task(s)": one of its two tasks is
  idle at its prompt and counts as paused).

## Test plan

- Integration (`crates/yulius/tests/self_update.rs`):
  `the_cancel_message_counts_every_task_still_in_a_turn` seeds 21 working tasks and one idle task,
  passes the bound and checks the full count in the run's error and that `waiting` shows 20;
  `the_cancel_message_counts_a_run_still_being_frozen` reaches the bound while a run is still being
  frozen and expects "1 task(s)"; `pausing_is_bounded_and_updates_can_be_cancelled` asserts the
  exact message. Before the fix the new tests read "20 task(s)" and "0 task(s)".
- Gates: `scripts/dev ci` and `scripts/dev security`.

## Decisions

- Count `progress.total.saturating_sub(progress.paused)`, as the issue asks: a run still being
  frozen at the bound is not paused, so it held the update back and belongs in the count, even
  though the page's list leaves it out. Raising `MAX_WAITING_SHOWN` or counting `waiting` without
  the cap was rejected: the list is a display bound and would still miss runs being frozen.

## Evidence

- Red: at 3168f583 (test only) `cargo nextest run -p yulius -E 'binary_id(yulius::main) &
  test(/^self_update::the_cancel_message/)'` failed with "20 task(s) were still in a turn…"; the
  frozen-run test, run against dev's `updates.rs`, failed with "0 task(s)". Green with the fix:
  `self_update::` 21 tests passed.
- `scripts/dev ci` at 3d67c511: green (2620 tests passed, 16 skipped; fmt, clippy, macOS, deps,
  doctests). The first run at b459d913 failed one unrelated test,
  `leftovers::kept_files_of_a_removed_sandbox_are_listed_and_deleted_on_request` (`kept[0].bytes`
  still null under load); it passed 5/5 alone and in the rerun. Not touched by this change; noted
  on #533 (this task creates no issues).
- `yulius security-scan` at 3d67c511: **not run** (exit 2): Vidoc's key is out of quota (HTTP 429,
  "used its 50,000,000 tokens"); `updates.rs` and `self_update.rs` were not scanned.
- Independent review (fresh session, om-code-review) of 91785a67..3d67c511: approve; its own-code
  findings were answered here (the frozen-run test, this Evidence, SPEC-160's back-link). Its
  pre-existing finding, a bound cancel on the pass that first finds every task paused ("0
  task(s)"), is outside this change and noted on #533.
