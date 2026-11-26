---
yulius-task: 1fbe203d-b576-4287-aa8e-b4c183b45e46
status: in-progress
---

# SPEC-667. Cleanup of delivery, the merge queue and the Git/GitHub adapters (audit L2)

## Goal

Less code that does the same thing in the delivery lane, in a shape a person can read: the
owner's November 2026 cleanup, audit report L2 (`L2-delivery.md`), plus the spec-discovery bug
that audit L10 found (F10).

## Context

The cleanup charter asks a chore for no spec. Yulius's edit gate holds code changes of this task
until it has one, so this spec stays lean: it records the plan and the evidence, and the audit
report is the design. Behaviour stays the same in every pull request, except the bug fix of
PR 0.

- `crates/yulius-app/src/delivery.rs` (8.2k lines) holds ten responsibilities (F8).
- The queue's two drivers (`agent_wait.rs`, `merge_train.rs`) and `pr_watch.rs` copy the
  readiness rule, the bring-up-to-date step and the armed-slot reads (F2, F5).
- "Merge now or arm" and "does the PR conflict with its base" are decided twice or three times
  (F3, F4).
- Queue state lives in three JSON files beside the store (F1).
- Spec discovery reads the first 500 directory entries before it sorts them; `.ai/specs` has
  more than 600 (L10 F10).

## Requirements

- **CLN-001** Spec discovery finds a task's spec in a specs directory of any size up to the
  listing bound, and prefers the file named with the task's branch slug.
- **CLN-002** Each later pull request keeps behaviour unchanged; a move is verbatim and in its
  own commit.
- **CLN-003** Nothing outside the task's claimed paths changes; `supervisor.rs` is untouched.
- **CLN-004** F7 (the parked-PR display reads) is not done: it drops a capability.

## Plan

- [ ] Spec discovery: failing test for >500 specs
- [ ] Spec discovery: sort before the cut, prefer the slug file
- [ ] Spec discovery: independent review answered
- [ ] Split delivery.rs: verbatim move into 8 files
- [ ] Split delivery.rs: independent review answered
- [ ] Queue: queue_readiness and armed_snapshot
- [ ] Queue: bring_armed_up_to_date for both drivers
- [ ] Queue: independent review answered
- [ ] Merge: preview_merge vs adds_to agreement test
- [ ] Merge: merge_or_arm and base_relation
- [ ] Merge: independent review answered
- [ ] Queue leftovers: typed dissolve reason, is_merged, branch filter, typed queue
- [ ] Leftovers/finish: TaskCache kind and background probe
- [ ] F6/F9/F10: independent review answered
- [ ] Adapters: branch check, test-only fns, hand-rolled code, Git builder repeats
- [ ] Adapters: port types (F11) within paths
- [ ] Adapters: independent review answered
- [ ] Queue state: armed merges, trains, notices into the store
- [ ] Queue state: independent review answered

## Acceptance criteria

- A task whose spec is one of 700 files, listed after 500 others, is found (CLN-001).
- Of two files naming the task, the one with the branch slug is registered (CLN-001).
- Every touched test binary passes unchanged apart from the tests of deleted code (CLN-002).

## Test plan

```sh
scripts/cargo nextest run -p yulius --test agent_hooks
scripts/cargo nextest run -p yulius --test delivery
```

## Decisions

- Discovery lists every name up to a bound far above any real specs directory and reads the
  slug-named file first, instead of sorting 500 entries: a sorted cut at 500 would still drop
  `SPEC-667` among 700 numbered specs.
