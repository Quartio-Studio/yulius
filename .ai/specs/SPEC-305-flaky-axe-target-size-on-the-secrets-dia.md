---
yulius-task: 08b0c486-2cb5-4f53-800a-1e68f0cbda66
status: in-progress
---

# SPEC-305: Toasts in a dialog never cover its Close (flaky axe target-size, #640)

Status: in-progress
Date: 2026-10-25
Owner: task #291
Scope authorization: issue #640 (opened by task #196), verified on `dev` at f770e4ff by task #289.

## Goal

Toasts shown inside a dialog never cover the dialog's Close button, including after the window
shrinks to the phone layout. The `secrets.e2e.ts` test "a failed copy shows no check, and long
notes wrap and scroll in their box" then passes every time.

## Context

- Symptom (#640): that test's last `axeViolations` check sometimes reports
  `serious target-size: button[aria-label="Close"]` (1 failure in 5 runs).
- Reproduced on f770e4ff with a probe that logs the boxes before axe and a pause before the
  test's `setViewport(390)`:
  - With no pause, or a pause of 1.5 s or 2.5 s: 3 toasts ("Value copied", "Couldn't copy" 1 and
    2), the lowest Dismiss at y 159–183. The dialog's × is at x 332–360, y 199–227. Pass.
  - With a 3.5 s pause: a 4th toast, the mock's ambient browser sign-in request ("#10 AWS login
    for deploy needs your action…", sent 5 s after the mock starts, the toast of #349). It arrives
    while "Value copied" (5 s) is still up. Its Dismiss is at x 351–375, y 209–233, on the
    dialog's ×. **Fail.**
  - With a 4.5 s or 5.5 s pause: "Value copied" is gone, so 3 toasts again. Pass.
  The flagged button is the **dialog's own ×**. Toast close buttons are named "Dismiss".
- Root cause, in the product:
  - While a modal is open, the toast region moves into it (`dom.js` `toastHost`).
  - At 1440 px the region sits bottom-right, clear of the dialog. Below 768 px
    (`styles.css` media query) it is pinned top-right, where the dialog's header × is.
  - `avoidFocus` moves the region to the other edge only when it covers the *focused* control.
    It runs only when a toast is added or focus moves, never when the window is resized.
  - So on a phone, or a window made narrow, enough toasts cover a dialog's only way out (WCAG
    2.5.8 and 2.4.11).
- Why the tests missed it: no test raises 4 toasts in a dialog at phone width. The flaky test gets
  a 4th toast only when the run is slow.
- What else the cause could break: any dialog with toasts at phone width. The focus avoidance
  itself could flip the region onto a control as covered as before. It now flips only to a clear
  edge.

## Requirements

- TDC-001: while the toast region is inside a dialog, it does not cover the dialog's header Close
  button, nor the focused control (as before). When it would cover either, it moves to the other
  edge. It stays where it is when the other edge would cover one of them too.
- TDC-002: the check runs when a toast is added, when focus moves and when the window is resized.

## Plan

- [x] Reproduce the flake and measure which Close is flagged
- [x] Spec SPEC-305 written and valid
- [x] Regression test that fails on the race
- [x] Toasts never cover a dialog's header Close (fix)
- [x] 20 consecutive passing runs recorded
- [x] Docs, changes.md and gates green
- [ ] Independent review answered
- [ ] Merged into dev on green CI

## Acceptance criteria

- The regression test (4 toasts raised in the secret dialog at 1440 px, then 390 px) fails on
  f770e4ff with the toast region over the ×, and passes with the fix.
- The original test passes with the probe's 3.5 s pause, which forces the 4-toast state.
- 20 consecutive runs of the original test pass.
- The e2e suite and `scripts/dev ci` pass.

## Test plan

- `e2e/tests/secrets.e2e.ts`: "toasts in a dialog keep off its Close, also after the window
  narrows". It raises 4 failed-copy toasts at 1440 px, which nothing moves: they sit bottom-right,
  and the copies are clicked from script, so focus does not move. It then switches to 390 px and
  asserts that the toast region does not overlap the dialog's ×, and that axe finds no violations.
- Each half of the fix is shown to be needed: the test fails without the resize check, and fails
  without the dialog Close in the check.
- 20 runs in a row of the original test, and runs with the 3.5 s pause.

## Decisions

- Product first, no test-only wait: the overlap is real for a person on a phone. A wait for the
  toasts to close (8 s) or `browserask=0` in the test would hide it.
- Only the dialog's header Close is kept clear, not every control of the dialog: any placement of
  the toasts covers some of a dialog's content on a phone. The Close is the way out, and focus
  avoidance (WCAG 2.4.11) already keeps the control being used clear.
- A window resize re-runs the check. A `ResizeObserver` on the region would also fire on every
  toast added or removed, which is already covered.
- No ADR: this extends the existing toast placement rule, and no lasting decision changes.

## Evidence

- Reproduction on f770e4ff: the boxes are in Context. Pauses of 0, 1.5, 2.5, 4.5 and 5.5 s
  passed. A 3.5 s pause failed with `serious target-size: button[aria-label="Close"]`, the
  issue's message.
- Red: `npx e2e run tests/secrets.e2e.ts --grep "toasts in a dialog keep off"` at 7f08742e (the
  test only) failed 3 of 3 with `expected {"toasts":4,"covered":true}` after `setViewport(390)`.
- Each half of the fix is needed (mutations of `dom.js` at 511ac118, then reverted):
  - Without the resize listener: red (`covered: true`, line 609).
  - Without the dialog's Close in `keepClear`: red as well, since the test takes the keyboard
    off the Close. While the Close had the keyboard (the dialog's first focus), the old focus rule
    already kept it clear, so the first version of the test missed this half.
  - With the fix: green.
- The original test with the probe's 3.5 s pause (4 toasts when axe runs) passed 3 of 3 with the
  fix. The region moved to the bottom edge (`flipped`), and axe found no violation.
- 20 consecutive runs of `--grep "a failed copy shows no check"` at 511ac118: 20 passed, 0 failed,
  5.2 s to 9.4 s each. Most ran past 5 s, the window where the mock's sign-in toast arrives.
- `npx e2e run` (full suite) at fc3a5349: 66 files passed, 1 skipped; 708 tests passed, 1 skipped.
- `scripts/dev ci` at fc3a5349: exit 0. Nextest: 2611 passed, 16 skipped.
- `scripts/dev security` at fc3a5349: exit 2, not run. Vidoc answered HTTP 429 ("This API key
  has used its 50,000,000 tokens"), so `dom.js` and `secrets.e2e.ts` were not scanned. Unrun, not
  passed.
