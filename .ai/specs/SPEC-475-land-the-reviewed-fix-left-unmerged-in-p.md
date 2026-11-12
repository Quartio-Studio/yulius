---
yulius-task: 21cf8c86-c140-427a-a833-2da9c188a2e1
status: in-progress
---

# SPEC-475: The reviewed fix of pull request #1162 reaches `dev` (issue #1079)

Status: in-progress
Date: 2026-11-12
Owner: Yulius task #457, issue #1079
Scope authorization: the request of task #457 (from task #430, which works through open issues):
deliver the finished fix of pull request #1162 in one pull request that says `Fixes #1079`, write
no second fix, comment progress on the issue, open no new issue.

## Goal

The fix for the flaky `local_ci::the_security_job_and_the_others_never_cancel_each_other`, written
and reviewed by task #436 and never merged, is in `dev`: the same test change, proven again on
today's `dev`, delivered by one pull request that closes issue #1079.

## Context

- **The fix and its record are [SPEC-454](SPEC-454-local-ci-the-security-job-no-cancel-test.md).**
  Symptom, root cause, the probes, the mutations, the decisions and three review rounds are
  there and are not repeated or rewritten here. In short: the test queued two runs whose held
  jobs competed for the fixture's three job slots under an 8 s job limit; now the other runs
  pass before the test holds any job, the test asserts that, and its fixture's job limit is an
  hour. Only `crates/yulius/tests/local_ci.rs` changes, nothing in the product.
- **Why it was not in `dev`.** Pull request #1162 (head `d1835270`) was approved for exactly
  `6022027e` (the two commits after it only merge `dev`), `yulius/ci` passed at its head, and
  merge train #1162 passed too (run `d85af3b4`). Task #436 was closed two minutes after that
  train started (2026-11-11T15:36:54Z), and a closed task's pull request is not merged when its
  run passes. The pull request stayed open with no running task; `dev` moved 23 commits and
  GitHub shows it as conflicting, in `docs/changes.md` alone.
- **The defect is still on `dev`** (`2a31d50a`): `local_ci.rs` is untouched there since the
  pull request's merge base (`bbd65665`), so the test still gets the fixture's 8 s limit and
  still queues the two competing runs.
- **What this task adds to the fix:** nothing in the test. It merges the pull request's head,
  moves the change log's entry to the day the fix lands, proves the fix on today's `dev` and
  delivers it.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| LND-001 | The delivered test change is the reviewed one: against `dev`, `crates/yulius/tests/local_ci.rs` differs exactly as it did at the approved `6022027e` against the `dev` of that time, and SPEC-454 is that head's file, unchanged. | `git patch-id` of both diffs; `git diff 6022027e HEAD -- .ai/specs/SPEC-454-*` empty |
| LND-002 | `docs/changes.md` has the entry for #1079 once, at the top (the day it reaches `dev`), and no other line of the file differs from `dev`. | `git diff origin/dev -- docs/changes.md` |
| LND-003 | The fix holds on today's `dev`: the test passes repeatedly, its guard still fails without the fix, the other `local_ci` tests pass beside it, and every gate passes on the merged head. | the runs and the gate in Evidence |
| LND-004 | One pull request delivers it and says `Fixes #1079`; pull request #1162 is closed with a pointer to it, and its branch is not left stale. | the pull request, #1162, `repo_hygiene` |

## Plan

- [ ] Pull request #1162's head merged onto this branch, the test and SPEC-454 as reviewed
- [ ] The #1079 entry of `docs/changes.md` stands once, at the top of the day it lands
- [ ] The fix proven on today's `dev`: the test repeated, its guard red without the fix, its neighbours
- [ ] Every gate run on the merged head
- [ ] What differs from the approved `6022027e` laid out for the independent review

After the merge, not Plan items (`delivery_merge` needs every step done first): the pull request's
description says `Fixes #1079`, pull request #1162 is closed with a comment that names its
replacement, issue #1079 gets a comment that says what was done, and `repo_hygiene` shows nothing
of this task or of #436 left.

## Acceptance criteria

- `dev` contains the test change of pull request #1162, unchanged, through one merged pull
  request whose description says `Fixes #1079`; issue #1079 is closed by it.
- The test passed repeatedly and in the full gate on a head that contains today's `dev`, and
  Yulius CI passed on the merged head.
- An independent review approved the delivered head, having judged what differs from
  `6022027e`.
- Pull request #1162 is closed and points at its replacement; no second fix was written.

## Test plan

- **The test, on the merged head:** `cargo nextest run -p yulius -E 'binary_id(yulius::main) &
  test(=local_ci::the_security_job_and_the_others_never_cancel_each_other)'`, repeated at rest.
- **The guard still guards on today's `dev`** (a mutation, never committed): without the block
  that lets the other runs go first, the test must fail on the assertion "every other run must
  have passed before this one holds its jobs". SPEC-454's probes (the slow polls, the forced
  slot order, the load runs) are not derived again: `local_ci.rs` and the code under it that
  they exercised are compared with the head they ran on instead (see Evidence).
- **The neighbours:** every `local_ci` test, several runs.
- **Gates:** `scripts/dev ci` on the merged head; `yulius security-scan` (its state is recorded:
  a disabled scan is not a passed one).
- **Yulius CI** on the pull request's head, before the merge.

## Decisions

- **Merge the pull request's head instead of copying its diff.** `git merge d1835270` keeps the
  fifteen reviewed commits and their messages, so "the test is the reviewed one" is a fact Git
  shows (`git diff d1835270 HEAD -- crates/yulius/tests/local_ci.rs` is empty), and the squash
  merge into `dev` makes one commit of it either way. A cherry-pick or a copied diff would have
  to be compared by hand. Alternative, reviving #1162 itself (merging `dev` into its branch):
  rejected, because that branch and pull request belong to a closed task: Yulius delivers and
  merges only the pull request of a running task's own branch, which is how #1162 was stranded.
- **SPEC-454 stays as it is.** It is the fix's record, `status: done`, with task #436's id. This
  spec is the delivery's record and points at it; nothing is copied, so the two cannot drift.
- **The change log's entry moves to the top of 2026-11-12 and names both tasks.** The file is
  newest first by the day a change reaches `dev`. The union merge left the entry where task #436
  wrote it, in the middle of 2026-11-11, between entries that landed that day, which would date
  the fix a day before it was in `dev`. Its text is task #436's; only the reference gains the
  delivering task and this spec.
- **The probes and mutations of SPEC-454 are not run again**, as the request says. What can
  have changed since they ran is `dev`, not the test, so the proof on today's `dev` is the test
  repeated, one mutation that shows the guard still fails without the fix, the neighbours and
  the full gate.
- **No decision record.** Nothing in the product or in the project's rules changes.
