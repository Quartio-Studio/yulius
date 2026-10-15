---
yulius-task: 869c8c44-10c2-4281-b069-018137e3dfdb
status: draft
---

# SPEC-169: The pull request chip stays on one line in a crowded step strip

Status: draft
Date: 2026-10-15
Owner: Yulius task #161, issue #542
Scope authorization: task #161: two screenshots of the task strip's "PR #523" chip broken onto two
lines over its pill, and "fix this pr thingy".

## Goal

In the task view's step strip, the "PR #N" chip keeps its text on one line inside its pill at every
strip width. The step label and the state give way first, as they already do (#341, UI-046).

## Context

- **Symptom.** With a panel open beside the task (Superagent, or a tool), the strip's PR chip
  shrinks below its text width. "PR" and "#523" then stack and overflow the 22 px pill. Reproduced in
  the mock: task #13 (PR #520) with the Shell panel open, window 1360 and 1400 px wide (strip 459 and
  499 px). `.strip-pr` shrinks from 82 to 64 px, and its `scrollHeight` 29 exceeds its
  `clientHeight` 22. The same happens at 800-960 px (strip 140-300 px).
- **Root cause.** `.tl-pr-chip` (`crates/yulius-server/web/assets/styles.css`) has a fixed height
  but no `white-space: nowrap`. The strip (`.stepstrip-main`) is a flex row, and its items shrink in
  proportion to their size. So `.strip-delivery` shrinks along with the long state, down to its
  min-content, which is the width of "#523", and the chip's text wraps. The chain chips do not wrap
  because `.strip-extras` is `nowrap`.
- **Why the tests missed it.** `responsive.e2e.ts` "a task beside a tool panel keeps room for its
  step count in a narrow strip" sweeps exactly these widths with task #13. It checks only box
  overlaps and the step count, and the chip's box keeps its fixed height while its text overflows.
- The same class also styles the task list's PR chip (`dom.js` `prChip`). A chip there never
  wrapped, but it should not.

## Requirements

- **PRC-001.** The pull request chip (`.tl-pr-chip`) never wraps its text, in the strip or in the
  task list.
- **PRC-002.** In the step strip, no chip's text overflows its box at any width the narrow-strip
  test sweeps. The step count still stays inside its toggle, and no strip parts overlap.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- **AC-1.** The extended narrow-strip e2e test fails on `dev` (the PR chip's text overflows at strip
  459/499 px) and passes with the fix.
- **AC-2.** The full e2e suite and the project gate pass.

## Test plan

- Regression: `e2e/tests/responsive.e2e.ts`, in "a task beside a tool panel keeps room for its step
  count in a narrow strip": at every swept width, every chip in the strip has
  `scrollHeight <= clientHeight` and `scrollWidth <= clientWidth` (no wrapped or cut text).
- Full e2e suite and `scripts/dev ci` for the gate.

## Decisions

- **`white-space: nowrap` on `.tl-pr-chip`, not `flex: none` on the delivery slot.** `nowrap` makes
  the chip's min-content its full text, so the flex row can no longer squeeze it, wherever the class
  is used. `flex: none` on `.strip-delivery` would fix only the strip and leave the class able to
  wrap elsewhere. No ADR: this is a local style fix, not a lasting decision.
- **What else the cause touches.** Any fixed-height chip in a shrinking flex row without `nowrap`
  can do the same. The other strip chips sit in `.strip-extras` (`nowrap`), and `.strip-status` is
  `nowrap` and clips its text with an ellipsis, so only the PR chip was exposed.
