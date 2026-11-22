---
yulius-task: a174b0f9-0ebc-473f-9d2f-77cfd2589379
status: in-progress
---

# SPEC-622 Finish #596: CI and Status repair actions for unowned gate failures

## Goal

Deliver the last step of task #596 ([SPEC-612](SPEC-612-a-failed-after-merge-gate-wakes-only-the.md),
GF-005): when an after-merge gate failure has no working task to own it, the project's people
see it on the project CI page and in the Status panel, with the failing test, the merge that
introduced it and the actions to run it again or start a task.

## Context

Task #596 ran on Codex and was stopped on 22 Nov 2026 to move to Claude Code. Eleven of its
twelve steps merged into dev (PRs #1675, #1678, #1682). PR #1682 delivered the API: the CI
overview (`GET /api/sandboxes/{sid}/ci`) and each sandbox of `GET /api/status` carry
`gate_notices` (`test`, `revision`, `run_id`, `merge_task`), non-empty only while the project
has an unresolved integration failure and no available repair worker
(`project_gate_notices`, `crates/yulius-app/src/ci/gate_history.rs`). The UI that reads it
waited on branch `feat/596-a-failed-after-merge-gate-wakes-only-the` at `03be88d24`, unmerged.
This task carries that slice onto current dev, re-verifies it and delivers it.

## Requirements

- CSR-001 Project CI shows a notice above the runs listing each unresolved test with the short
  revision that first showed it and its merge task (or "untracked merge").
- CSR-002 The Status panel shows the same notice for every visible project with one, and does
  not claim "Everything is running normally" beside it.
- CSR-003 Writers get "Review and run again" (opens the latest affected run, whose Run again
  retries it) and "Start a task" (the project's composer); readers get only "Review the run".
- CSR-004 Refreshes keep the keyboard where it was: a moved or cleared action never hands its
  focus to another project's action, and an older Status response never restores a notice a
  newer one cleared.

## Plan

- [x] Bring the step-12 CI and Status notice slice onto current dev
- [ ] Prove the Chromium notice tests fail without the change
- [ ] Run the touched e2e specs and the web unit checks on current dev
- [ ] Review the slice against the current API and fix own findings
- [ ] Independent review of the delivery and its fixes
- [ ] Record evidence and close SPEC-612's last step

## Acceptance criteria

- With a gate notice for a project, project CI and Status list each failing test, its
  introducing revision and task, and writers can open the run and run it again or start a task.
- A reader sees the notice without either write action.
- Each Chromium test of the notice fails on dev without the UI change and passes with it.

## Test plan

Chromium tests in `e2e/tests/ci.e2e.ts` against the mock (`gate_notice=1` for a writer with an
inherited and a new failure, `gate_notice=reader` for a reader with an untracked merge), the
focus and overlapping-poll cases, and the cancelled-gate verdict. Run `ci.e2e.ts`,
`task-done.e2e.ts` and the Status-related specs in full; revert the UI files to dev and rerun
`ci.e2e.ts` to show the new tests fail. The backend contract is already covered by
`crates/yulius/tests/staging.rs` and `parking.rs` (merged in PR #1682).

## Decisions

- Carry the old branch's three-dot diff as one commit onto a fresh branch from dev instead of
  merging the old branch: its history holds commits already squashed into dev through three PRs,
  and a merge would replay their conflicts for no benefit.
- One set of actions per project, pointing at the latest affected run (`notices[0].run_id`):
  every notice of a project shares the newest red run, which retries all open tests at once;
  per-test actions would offer duplicate links to the same run (SPEC-612 evidence).
- No ADR: the decision behind the notice is ADR-0450; this delivers its presentation.

## Evidence

Filled in at the end.
