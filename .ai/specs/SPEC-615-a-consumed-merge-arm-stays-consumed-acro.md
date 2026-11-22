---
yulius-task: ba761fad-f0e6-444f-93e9-4e32a3b23069
status: done
---

# SPEC-615 A consumed merge arm stays consumed across a restart

## Goal

A merge activation that the ordinary merge queue already decided is never armed again by a
restart, even when removing its entry from `armed-merges.json` failed. A later re-arm of the same
head keeps its own place, and an unreadable verdict keeps the saved activation recoverable.

## Context

Issue #1564. The ordinary queue (`merge_armed_pass`, `finish_armed`) finishes a decision in
memory, tries to clear the saved arm (`save_armed`, a failure is only logged) and records the
final wait as a finished `wait`/`merge_armed` span stamped with the activation's `armed_at`
(`armed_wait_ended`). At startup `restore_armed_merges` compares a saved entry with the latest
finished merge wait only when that span carries a singleton receipt (SPEC-558). An ordinary span
has none, so a leftover entry of a decided activation was armed again for a live task, and the
next pass decided it a second time over the real outcome.

Every activation gets an `armed_at` later than every earlier activation and every recorded merge
wait of the task (`arm_merge_as`: `max(now, last_armed_at + 1)`, with `last_armed_at` raised to
the latest merge wait's start). Ordinary and singleton waits are both recorded only for a final
decision, stamped with the decided activation's `armed_at`.

## Requirements

- **CMA-001** At startup a saved activation whose `armed_at` is not later than the start of the
  task's latest finished merge wait is consumed: its entry is cleared and nothing is armed,
  whether the verdict came from the ordinary queue or from a singleton decision, and whatever its
  readable detail says.
- **CMA-002** A saved activation later than that wait (a re-arm of the same head after the
  verdict) is restored with its own `armed_at`, so it keeps its place in the queue.
- **CMA-003** When the latest merge wait or its detail cannot be read, the saved entry stays on disk, the
  task reads as armed to the readers of the saved arms (`merge_is_armed`: task views, scopes,
  parking, the PR watcher) and arm changes are refused, until a later restore reads the verdict and
  reconciles it.

## Plan

- [x] Reproduce the restored stale arm after a refused file clear
- [x] Consume a saved activation decided by any final merge wait at startup
- [x] Prove a later re-arm keeps its place and an unreadable verdict stays recoverable
- [x] Record the decision, architecture note and change entry
- [x] Independent review approves the change

## Acceptance criteria

- **AC-1** An ordinary final verdict, a refused clear of the saved arm and a fresh application
  restoring the original saved entry: the task is not armed and the entry is cleared.
- **AC-2** After that verdict a re-arm of the same head survives a fresh application's restore,
  armed with the `armed_at` it was saved with.
- **AC-3** With the merge wait unreadable, a fresh application keeps the entry, reads the task as
  armed and refuses a re-arm; once the record is readable again, the next restore consumes the
  entry and a re-arm gets a new place.

## Test plan

Integration tests in `crates/yulius/tests/delivery.rs` with the `ArmedTask` fixture (real Git,
the in-memory store, the fake GitHub): a directory at `armed-merges.json.tmp` refuses the file
clear, a moved branch gives the ordinary final verdict, `App::new` on the same config and store
is the fresh process, and `execute_for_tests` damages the span's detail (a blob the row decode
refuses, then malformed JSON) and puts it back. AC-1 and AC-3 fail without the change; AC-2 guards the boundary.

## Decisions

The restore treats any finished merge wait at or after the activation stamp as consuming it
(ADR-0452), instead of making the ordinary queue hold its verdict until the file clear succeeds
as `finish_singleton` does: the verdict is already the durable record, the restore already
consumes singleton verdicts by the same comparison, and an agent waiting on the decision is not
kept waiting by a file-system error. The ordinary verdict is still written best-effort after
the clear; a restart after both writes fail restores the arm (issue #1691, outside #1564). The verdict is read in full before the comparison: an
unreadable detail keeps the entry recoverable (CMA-003), as SPEC-558 already does for a damaged
singleton receipt.

## Evidence

- Reproduced first (commit `79c6acb26`, tests only):
  `scripts/cargo nextest run -p yulius --test main -E 'test(/saved_arm_when|later_rearm_of_the_same|unreadable_verdict_keeps/)'`.
  `an_ordinary_verdict_consumes_its_saved_arm_when_the_clear_failed` failed with "the consumed
  activation was armed again". `an_unreadable_verdict_keeps_the_saved_arm_until_it_is_reconciled`
  failed with "came back after: …". `a_later_rearm_of_the_same_head_keeps_its_place_across_a_restart`
  passed (boundary guard). With the fix reverted on the final tests, the same two fail.
- After the fix: the three tests pass. `-E 'test(/^delivery::|^parking::|^merge_train::|^antigravity_armed_merge::/)'`
  ran 382 tests, 382 passed. That includes SPEC-558's
  `a_damaged_singleton_receipt_keeps_the_consumed_arm_in_recovery`, which failed while the stamp
  comparison came before the verdict read. `scripts/cargo nextest run -p yulius-app --lib`: 700
  passed. `cargo fmt --check` and `clippy -p yulius-app -p yulius --all-targets -D warnings` are
  clean, and `scripts/check_repository.py` passed.
- Yulius CI run `fb083151` on head `b498d5ada` (PR #1686) passed: quality, lint, macos, docs, deps
  and file-contracts. The test shards and e2e run on the merged revision (merge-first). The
  security scan is disabled for this project; it was not run.
- Independent review (round 1, yulius-reviewer) approved `b498d5ada`. It stress-ran the three
  tests 40 times with no failure and found no path that records a merge wait at or after a live
  activation's stamp. Its notes were answered:
  - the conflict assertion now requires "still being restored";
  - ADR-0452 and CMA-003 name their limits and readers;
  - pre-existing issues were filed: #1691 (best-effort verdict write, `settle_armed` clear) and
    #1690 (flaky `a_candidate_refresh_waits_for_the_launch_to_reserve_its_verified_slot`, 3/40 on
    dev's code).
