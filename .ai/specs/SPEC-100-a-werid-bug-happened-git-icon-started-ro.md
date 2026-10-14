---
yulius-task: 6ab5b465-1674-4e17-a8c7-19a85ab4de56
status: done
---

# SPEC-100: a task waiting for Yulius shows a still, muted branch

Status: done
Date: 2026-10-14
Owner: task #85
Scope authorization: the person's request (task #85): "A werid bug happened, git icon started
rotating and became green lol. Fix." Screenshot: the sidebar's Working group, where the first row
("Menus show an optio…", working <1m) has a green git-branch icon among the turning rings.
Issue: #393.

## Goal

A task whose agent waits in `yulius wait` for its checks or merge shows its branch icon standing
still, in the muted tone of the other waiting states, instead of a green branch that spins like
the Working ring. It stays in the Working group and still notifies nobody.

## Context

- **Symptom.** In the sidebar (and the Tasks page and the task header, which use the same
  `statusMark`), a task "Waiting for checks or merge" shows a green, turning git-branch icon.
- **Root cause.** SPEC-085 STT-002 gave this state (`WAITING_ON_YULIUS` in
  `crates/yulius-server/web/assets/status.js`) the branch icon with `tone: 'working'`, so that a
  task starting to wait notifies nobody (`TASK_TONES` in `notifications.js` notifies only on
  waiting, done, attention, failed and stopped-abnormal). But `styles.css` colours
  `.tone-working` with `--ok` (green) and rotates every `.glyph.tone-working .icon`: the rule was
  written for the `st-working` ring, which is drawn to spin. The branch got both.
- **Why the tests missed it.** The STT-002 e2e test checks the state's label, group and the
  attention bar, not how its mark looks.
- **Other paths through the same value.** Failing checks while the agent waits in `yulius wait`
  (STT-003) spread `WAITING_ON_YULIUS` too ("Waiting for checks or merge · PR #N checks failing"),
  so they get the same still, muted branch. A task with background shells keeps the Working ring
  (`stillWorking`), which does turn.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| WTI-001 | The status mark of a task whose idle agent waits in `yulius wait` shows the branch icon still (no animation), in the muted tone of Queued, Chained and Paused, not Working's green. It stays in the Working group and its tone notifies nobody. This amends SPEC-085 STT-002's "with the tone of Working". | e2e `task-done.e2e.ts` "an agent waiting for Yulius shows a still, muted branch"; "an agent waiting for Yulius is Working, not your turn" (group) |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- With task #12 waiting in `yulius wait` in the mock studio, its sidebar mark's icon has
  `animation-name: none`, and its colour differs from a Working task's turning ring.
- The task stays under Working, not Needs you; the other `task-done.e2e.ts` tests still pass.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- e2e (`task-done.e2e.ts`) "an agent waiting for Yulius shows a still, muted branch": it reads the
  computed animation and colour of the waiting row's mark beside a Working row's. It failed before
  the fix ("expected "spin" to be "none"") and passes after.
- No notification: `queued` is not in `TASK_TONES`, so the tone change notifies nobody, as with
  `working`. The suite has no notification harness for task tones; the reasoning is recorded here.
- The whole e2e suite and `scripts/dev ci` for regressions.

## Decisions

- **The muted `queued` tone, not a new one or a still green.** ADR
  [0080](../../docs/decisions/0080-a-wait-for-yulius-shows-a-still-muted-branch.md). A wait for
  checks is a wait like Queued, Chained and Paused, which each have their own shape in the muted
  tone and stand still (#147); `queued` notifies nobody, which is what STT-002 needed from
  Working's tone. Alternatives: keep `working` and stop only the branch from spinning (it stays
  green, the colour of Working and Done, so the row still reads as busy or finished); a new tone
  (more CSS for the same look); the Working ring (it hides that the agent waits for Yulius, which
  STT-002 shows on purpose).

## Evidence

- **Red first.** `npx e2e run tests/task-done.e2e.ts` with the test alone (commit ef3cf12d): 12
  passed, 1 failed, "an agent waiting for Yulius shows a still, muted branch": `expected "spin" to
  be "none"`. With the fix (f83881e9): 13 passed.
- **Gates.** `scripts/dev ci` exit 0: repository contracts, unit scripts, `cargo check`, `fmt`,
  `clippy -D warnings`, `nextest` 1520 passed (16 skipped), doctests. The whole e2e suite
  (`E2E_TELEMETRY_DISABLED=1 npx e2e run`): 41 files passed, 1 skipped; 282 tests passed, 1
  skipped. After merging `origin/dev` (#396, which touched `styles.css`): `task-done.e2e.ts` and
  `loading.e2e.ts` 18 passed.
- **Review.** An independent read-only session (`om-code-review`) approved f83881e9. It checked
  every consumer of `info.tone` (notifications, dashboard outcome, strip status, Tasks page, task
  time), the mock's stable Working row the test compares with, and other states pairing an icon
  with a turning tone (none). Its minor finding (SPEC-085's Design line still said tone `working`)
  and a label nit in STT-003 were fixed in 2545aee0.
- **Not tested here.** The live sidebar of a real `yulius wait` task; the mock drives the same
  `statusInfo` and `statusMark` code.
