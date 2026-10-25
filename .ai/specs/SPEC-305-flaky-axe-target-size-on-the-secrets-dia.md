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
  itself could flip the region onto a control as covered as before. It now flips only to an edge
  that costs less (TDC-001).

## Requirements

- TDC-001: the toast region keeps clear of the focused control (as before) and, while it is
  inside a dialog, of the dialog's header Close button. The focused control comes first: an edge
  costs 2 when it covers the focused control and 1 when it covers the Close. The region moves to
  the other edge only when that edge costs less. So it never moves onto the focused control to
  clear the Close, and it stays put when the other edge is no better.
- TDC-002: the check runs when a toast is added, when focus moves and when the window is resized.

## Plan

- [x] Reproduce the flake and measure which Close is flagged
- [x] Spec SPEC-305 written and valid
- [x] Regression test that fails on the race
- [x] Toasts never cover a dialog's header Close (fix)
- [x] 20 consecutive passing runs recorded
- [x] Docs, changes.md and gates green
- [x] Independent review answered
- [ ] Merged into dev on green CI

## Acceptance criteria

- The regression test fails on f770e4ff with the toast region over the ×, and passes with the
  fix. It raises 4 toasts in the secret dialog at 1440 px, then narrows the window to 390 px.
- With the dialog's Done focused while the toasts at the bottom cover it, they move to the top
  and stay there on the next check, although the top covers the Close.
- The original test passes with the probe's 3.5 s pause, which forces the 4-toast state.
- 20 consecutive runs of the original test pass.
- The e2e suite and `scripts/dev ci` pass.

## Test plan

- `e2e/tests/secrets.e2e.ts`: "toasts in a dialog keep off its Close, also after the window
  narrows". It raises 4 failed-copy toasts at 1440 px, which nothing moves: they sit bottom-right,
  and the copies are clicked from script, so focus does not move. It then switches to 390 px and
  asserts that the toast region does not overlap the dialog's ×, and that axe finds no violations.
  It then focuses Done, which the toasts at the bottom cover. The toasts move to the top (over the
  Close), and they stay there when a resize event re-runs the check. After the dialog closes (the
  toasts back in the page), a focused control that both edges cover leaves them where they are,
  on focus and on resize.
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
  toast added or removed, which is already covered. The check does not run again when the dialog's
  own layout changes, such as content revealed that moves its Close up (review nit). Such a change
  usually comes with a click or a focus move, which re-runs it.
- The focused control comes before the Close (review round 1): keyboard users must see the control
  they are on (WCAG 2.4.11). A pointer user can still dismiss the toasts or wait for them to close.
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
- Review round 1 (fresh session, head 433bbad6): request changes. Answers:
  - Major, own code: the toasts could settle over the focused control to clear the Close. Fixed
    test-first. The test that focuses Done failed at 433bbad6 with
    `{"toasts":4,"close":false,"done":true}` (2 of 2), and passes with the cost rule at 3b552402.
  - Minor, own code: the "stay" rule had no test. Added the check after a resize event. It fails
    with the old unconditional toggle (line 625) and passes with the fix.
  - Minor, own code: one read right after `setViewport`. It now waits with `expect.poll`.
  - Nit, own code: no re-check on the dialog's own layout changes. Accepted in Decisions.
  - Nit, pre-existing: `app.js` teardown and `spotlight.js` remove a dialog with the toasts still
    inside, so the next toast makes a new region with new listeners. Harmless: the old region is
    detached and never moves. Reported on #640. `host.parentElement?.` was added as a guard.
- At 3b552402, the original test with the 3.5 s pause passed 3 of 3. "Copy notes" had the keyboard
  and the toasts moved to the bottom, clear of it and of the Close.
- After round 1, at e5aa60e1:
  - 20 consecutive runs of `--grep "a failed copy shows no check"`: 20 passed.
  - `npx e2e run` (full suite): 708 passed, 1 skipped.
- After merging origin/dev (0f903217, which brings SPEC-308's changes to `secrets.js` and
  `secrets.e2e.ts`), at 72b7d599:
  - `npx e2e run tests/secrets.e2e.ts`: 27 passed.
  - `scripts/dev ci`: exit 0. Nextest: 2612 passed, 16 skipped.
  - `scripts/dev security`: exit 2 again, not run (Vidoc HTTP 429, quota). Unrun, not passed.
- Review round 2 (fresh session, head 72a01c68): request changes. Answers:
  - Minor, own code: the "stay when no better" case (equal cost) had no test. A `>` mutant passed.
    Added the check after the dialog closes: a full-height focused button that both edges cover.
    The `>` mutant now fails at line 711 (`moved: [true, false]`: it jumps on focus and back on
    resize). The fix passes.
  - Nit, own code: Context said "flips only to a clear edge". It now says "an edge that costs
    less".
  - Nit, own code: the `docs/changes.md` wording ("ended up there … nothing moved them") and its
    wrapping are fixed.
  - `npx e2e run tests/secrets.e2e.ts` after these changes: 27 passed, 3 runs of 3.
