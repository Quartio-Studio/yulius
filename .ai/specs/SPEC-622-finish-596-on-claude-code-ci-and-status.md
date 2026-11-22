---
yulius-task: a174b0f9-0ebc-473f-9d2f-77cfd2589379
status: done
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
- [x] Prove the Chromium notice tests fail without the change
- [x] Run the touched e2e specs and the web unit checks on current dev
- [x] Review the slice against the current API and fix own findings
- [x] Record evidence and close SPEC-612's last step
- [x] Independent review of the delivery and its fixes
- [x] Answer every independent review finding on the exact head

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

- Carried slice: the three-dot diff of `feat/596-a-failed-after-merge-gate-wakes-only-the`
  (`03be88d24`) against dev `2a6e07423` applied cleanly as `cd6bab35f`.
- With the change, `npx e2e run tests/ci.e2e.ts` passed 31 of 31: 10 new notice cases beside the 21 on dev; the eleventh new assertion is the sidebar title in `task-done.e2e.ts`.
- Without it (`layout.js`, `pages/ci.js`, `dom.js`, `index.html` from dev, `gatenotices.js`
  removed; mock and tests kept), `npx e2e run tests/ci.e2e.ts tests/task-done.e2e.ts` failed 9 of
  59: the writer and reader notices, the Status health line, both two-project focus cases, the
  Sandboxes disclosure, the cleared CI link, the cancelled verdict and the sidebar title
  (`task-done.e2e.ts`). The two overlapping-poll cases pass vacuously there (no notice is ever
  drawn), so they were run against the slice with only `load()` reverted to dev's behaviour
  (accept any answer while a panel is open, draw into the current panel): both failed, then
  passed with the request-order guard.
- The full Chromium suite on the slice (`d74dd5846`): 1,335 passed, 1 configured skip, 99 files.
- `python3 scripts/check_repository.py`: repository contracts passed.
- Own review against dev's API: all notices of a project carry the same latest red run
  (`GateFailures::notices`), so one set of actions is complete; Status reads the person's access
  from the sandbox list; `keepFocus` behaves as before when no `missingKeyTarget` is given. It
  found `gatenotices.js` and its flows missing from the web and e2e READMEs (added).
- Vidoc is switched off for this project in Yulius: the security scan is disabled, not passed.
- The backend behind `gate_notices` (SPEC-612 GF-001 to GF-006) merged in PR #1682 with its Rust
  regressions; it is unchanged here.
- Independent review (Fable 5.2, round 1) approved `f09734cc3` after Yulius CI run
  `45063bba-f942-49cb-bb0c-d56cba396273` passed on it. It reported four minor findings and four
  nits, all in this change. The Plan had ticked the review before the report existed, so the
  answer is its own step.
  - Fixed test-first. Each new assertion failed on `f09734cc3`:
    - A cleared notice moved the keyboard to the CI overview, which then drew a 2 px focus frame
      around every card (`box-shadow ... 0 0 0 2px`). The overview now has the run view's
      no-frame rule (SPEC-321).
    - A focused "Review and run again" lost the keyboard to the section when a newer red run
      replaced the affected one. The link is now keyed by project and action, not by run.
    - A Status link to the page already shown left focus on `body`. It now returns to the
      Status button.
    - Axe now checks the writer and reader notices with Status open (it found no violations
      before or after).
    - After the fixes, `ci.e2e.ts` passed 33 of 33.
  - Docs: the changes entry is first in its day, and the web README lists `gate_notice=1|reader`.
  - Left as they are:
    - The `el === panel` half of the Status guard has no test. The reviewer found no visible
      failure without it, and it stops an answer from an earlier opening being drawn.
    - The sidebar title shows the staging outcome before the detail. Moving it would change a
      line of over 200 bytes after the approval.

