---
yulius-task: 911b66b2-e0d0-4e52-92ab-0fec1d3e047d
status: in-progress
---

# SPEC-532. Deliver follow-ups when the head contains the integration branch

## Goal

Deliver a second slice on the same task branch when it includes the fetched integration commit and adds work to its tree.

## Context

Task #473's first pull request merged at head `aa154aec4`. Git confirms integration commit `3523948f3` is an ancestor of follow-up `df00249f3`. Their trees differ. Replaying both appends to the migration list against the first pull request's head conflicts at migration 0059. Delivery then asks the task to merge a commit it already contains. The existing follow-up test adds a different file and misses this case. [Issue #1299](https://github.com/Quartio-Studio/yulius/issues/1299)

## Requirements

- **FUP-532-1** When a follow-up contains the fetched integration commit, classify differing trees as new work and equal trees as covered.
- **FUP-532-2** For divergent histories, retain the explicit three-way merge from the merged PR head, including the unknown verdict on conflict. Keep the existing delivery readiness and ancestry checks. [ADR-0269](../../docs/decisions/0269-work-after-a-tasks-merge-is-delivered-as-a-new-pull-request.md)

## Plan

- [x] Reproduce the migration append conflict and specify its verdict
- [x] Cover different and equal descendant trees in delivery tests
- [ ] Judge an integrated follow-up by its Git tree
- [ ] Verify and independently review the delivery fix

## Acceptance criteria

- **AC-1** After the first PR merges, dev appends A. The task incorporates dev and appends B at the same position relative to the first head. Delivery opens a follow-up for A plus B.
- **AC-2** A distinct descendant commit with dev's tree is reported as already merged and opens no PR.
- **AC-3** A divergent branch still reports conflicts and recognizes a follow-up already squash-merged elsewhere.

## Test plan

Use the delivery fixture with real Git and its GitHub test double for AC-1 and AC-2. Run the existing divergent-history tests for AC-3. Record the red regression before applying the fix.

## Decisions

Use the existing Git adapter comparison, which pins the integration ref to one commit. Ancestry makes the head tree the exact result of integrating that head. The application already consumes the adapter's verdict for both automatic delivery and `delivery_merge`.

## Evidence

- On the original code, `cargo test --locked -p yulius --test main delivery::a_follow_up_containing_dev_is_judged_by_its_tree -- --exact` failed at the second delivery call. It reported a conflict with `origin/dev` and advised merging it again. The equal-tree assertions passed.
