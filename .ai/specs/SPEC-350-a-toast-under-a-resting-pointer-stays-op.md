---
yulius-task: b8f73838-bcdd-4709-957b-9adf63a75be4
status: in-progress
---

# SPEC-350: A toast under a resting pointer closes on time

Status: in progress
Date: 2026-10-30
Owner: Yulius task #334
Scope authorization: task #334, fix GitHub issue #954 (flaky `secrets.e2e.ts` "a new agent secret
lets agents set it up…": a lingering toast covers "Add team secret").

## Goal

A toast pauses its close timer only when the pointer really moves onto it (or keyboard focus goes
into it). A toast that appears, or moves, under a pointer resting where it was closes on time, so
it never covers the page header's buttons for good.

## Context

- `toast()` (`crates/yulius-server/web/assets/dom.js`) clears its timer on `pointerenter` and
  `focusin` and arms it again on `pointerleave` / `focusout` when the toast is neither hovered nor
  focused (WCAG 2.2.1, since #896 and earlier).
- Chromium dispatches boundary events (`pointerover`, `pointerenter`, `mouseover`) to an element
  that appears under a still pointer, but no `pointermove`. A probe in the e2e harness showed it:
  a 500 ms toast raised under a parked pointer got `pointerover`, `pointerenter` and `mouseover`,
  was `:hover` and was still open 2 s later. Nothing ever sends the `pointerleave` that would arm
  it again, so it stays until the pointer moves.
- The stack moves without a pointer move in three ways: a new toast appears where the pointer
  rests, `keepClear` flips the stack to the top right (under the tab row, over the page header's
  `.page-actions` add button) and `toastHost` moves the host between a dialog and `<body>`.
- #954: in `secrets.e2e.ts` the "Agent secret added." toast flipped under the pointer and was still
  shown 30 s later, over "Add team secret", whose click timed out. #349 (SPEC-148 E2L-006) was the
  same mechanism over the Notes tabs; it was worked around in the tests (`browserask=0`), not in
  `toast()`.

## Requirements

| ID | Requirement |
| --- | --- |
| TST-001 | A toast that appears under a pointer resting where it was closes after its timeout. |
| TST-002 | A toast that the stack's flip (`keepClear`) brings under a resting pointer closes after its timeout. |
| TST-003 | A toast the pointer moves onto stays while the pointer is on it and closes once it leaves (WCAG 2.2.1, unchanged); keyboard focus inside it keeps it as before. |
| TST-004 | A toast under a resting pointer that keyboard focus enters and leaves closes after its timeout (the resting pointer does not count as pointing at it). |

## Plan

- [x] Spec SPEC-350 written and validated
- [x] Regression test shows a toast under a resting pointer staying open
- [x] Toast pauses only on a real pointer move onto it
- [x] Decision, changes and e2e README documented
- [ ] Gates green and independent review answered
- [ ] Pull request merged into dev on green

## Acceptance criteria

- `e2e/tests/toasts.e2e.ts` fails on the unfixed `toast()` (the resting-pointer cases) and passes
  with the fix; the pointer-moved-onto case passes on both.
- `secrets.e2e.ts` and the full e2e suite pass.

## Test plan

- `e2e/tests/toasts.e2e.ts` (mock page with `ambient=0&browserask=0`, toasts raised in the page
  through `dom.js`'s `toast()` with a 2 s timeout, 3 s for the two-toast case, so a loaded host acts
  inside it):
  - pointer parked at the bottom right, a toast raised there: it is `:hover`, then gone (TST-001);
  - a focused control placed where the stack shows, the pointer parked at the top right, a toast
    raised: the stack flips under the pointer, it is `:hover`, then gone (TST-002);
  - two toasts, the pointer moved onto one: the other closes, the pointed one stays, and closes
    once the pointer leaves (TST-003);
  - pointer parked at the bottom right, a toast raised there, its Dismiss focused and blurred: it
    is gone after its timeout (TST-004, review round 1).
- Full e2e suite, `scripts/dev ci`.

## Decisions

- [ADR-0237](../../docs/decisions/0237-a-toast-pauses-only-when-the-pointer-moves-onto-it.md): pause
  on `pointermove`, not `pointerenter`. Alternatives: check `:hover` when the toast is created
  (misses the flip and host moves after creation), wait the toast out in the test (fixes one test,
  leaves the product toast blocking the header for a person whose mouse rests there, and the next
  test that clicks the header).
- The test-side workarounds of #349 / #503 (`browserask=0`, the wait in `notes.e2e.ts`) stay: they
  also keep the sign-in toast off what those tests point at, and removing them is not this fix.
