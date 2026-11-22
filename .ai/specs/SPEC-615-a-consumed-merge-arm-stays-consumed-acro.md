---
yulius-task: ba761fad-f0e6-444f-93e9-4e32a3b23069
status: in-progress
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
  detail says.
- **CMA-002** A saved activation later than that wait (a re-arm of the same head after the
  verdict) is restored with its own `armed_at`, so it keeps its place in the queue.
- **CMA-003** When the latest merge wait cannot be read, the saved entry stays on disk, the
  task reads as armed and arm changes are refused, until a later restore reads the verdict and
  reconciles it.

## Plan

- [ ] Reproduce the restored stale arm after a refused file clear
- [ ] Consume a saved activation decided by any final merge wait at startup
- [ ] Prove a later re-arm keeps its place and an unreadable verdict stays recoverable
- [ ] Record the decision, architecture note and change entry
- [ ] Independent review approves the change

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
is the fresh process, and `execute_for_tests` turns the span's detail into a blob (row decode
fails) and back. AC-1 and AC-3 fail without the change; AC-2 guards the boundary.

## Decisions

The restore treats any finished merge wait at or after the activation stamp as consuming it
(ADR-0452), instead of making the ordinary queue hold its verdict until the file clear succeeds
as `finish_singleton` does: the verdict is already the durable record, the restore already
consumes singleton verdicts by the same comparison, and an agent waiting on the decision is not
kept waiting by a file-system error. Detail is not parsed for this comparison, so an unreadable
detail of a consuming span cannot revive the activation.
