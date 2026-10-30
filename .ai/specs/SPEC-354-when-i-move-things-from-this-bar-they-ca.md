---
yulius-task: c3ac7da8-9749-4a0b-96fe-99f2e6075f96
status: done
---

# SPEC-354: a dragged tab stays in its strip and bounces off its ends

Status: implemented
Date: 2026-10-30
Owner: Yulius task #336
Scope authorization: user request "when i move things from this bar, they can go far after the
available space. It should be blocked so visually it bounces from borders instead of going
infinitly far", with a screenshot of the sandbox tab row (yulius, quartio-v2, product,
pomerado-rust, studio, +) showing the six-dot drag grips.

## Goal

A tab dragged along its strip never leaves the strip: past either end it meets resistance, moves at
most a few pixels further, and on release springs back into its place.

## Context

- The sandbox tab row (SPEC-336), the Notes tabs (SPEC-112) and the Editor tabs (SPEC-127) all drag
  with `assets/tabdrag.js` (`createTabDrag`). `move` sets the pressed tab's
  `translateX(dx)` with `dx = clientX - startX`, unbounded: dragged left of the first tab or right
  of the last it follows the pointer over "All sandboxes", the `+` button, the panels, anywhere.
- On release the tab settles (FLIP) from where it was let go (`from = lefts[from] + dx`) into its
  slot, so the far-away tab flies back across the page.
- The slot is already bounded (`layout` counts the tabs whose centre is left of the dragged one), so
  only the drawn position is wrong.

## Requirements

| ID | Requirement |
| --- | --- |
| BND-001 | While dragged, the tab's box stays within the extent of its strip's draggable tabs (the first tab's left edge to the last tab's right edge), plus an elastic overshoot that grows ever more slowly with the pointer's distance past the edge and never exceeds `OVERSHOOT_MAX_PX` (16 px). |
| BND-002 | Inside that extent the tab follows the pointer exactly, as before. |
| BND-003 | On release past an edge the tab settles from its drawn (bounded) position into its slot, so it visibly springs back from the border rather than flying in from where the pointer was. |
| BND-004 | The order chosen is unchanged: it follows the pointer, not the bounded drawing, so dragging far past an end puts the tab at that end even when it is much wider than the tab there. |
| BND-005 | The same bound applies to the sandbox, Notes and Editor tab strips (one shared module). |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Failing e2e: a sandbox tab dragged past either end of the row stops at the border
- [x] Shared tab drag bounds the dragged tab with an elastic overshoot
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-354-1: dragging the first sandbox tab 400 px left of the row leaves it at most 16 px left of
  where the row starts; dragging the last one 400 px right leaves it at most 16 px past the row's
  last tab; the order is then first/last as expected and no sandbox opens.
- AC-354-2: the existing sandbox, Notes and Editor drag tests keep passing (inside the extent the
  tab still follows the pointer exactly).

## Test plan

- e2e `navigation.e2e.ts` "a sandbox tab dragged past either end of the row stops at its border":
  real mouse drags of the first tab far left and the last tab far right; mid-drag the dragged tab's
  bounding box stays within the row's tab extent ± 16 px and is past the edge by more than 0 (it
  gives a little); release reorders as expected, opens nothing, and the transforms settle to none.
- Regression: `navigation.e2e.ts`, `notes.e2e.ts`, `editor.e2e.ts` drag tests.

## Decisions

- Bound in `tabdrag.js`, not per strip: all three strips share it, and the user's "bar" is one of
  them; a strip-specific clamp would leave the other two flying.
- The extent is the draggable tabs' own (from the offsets measured at press), not the strip's
  scroll box: "All sandboxes" and `+` are not slots, and the tab never needs to cover them.
- Rubber band `max · over / (over + max)` (asymptotic to 16 px) rather than a hard clamp: the user
  asked for it to bounce from the borders; resistance plus the existing settle transition gives
  that bounce without a spring simulation. A hard clamp would feel stuck; an overshoot that keeps
  growing would not be "blocked".
- The slot is chosen from the pointer's own distance; only the drawn position (and the settle's
  start) is bounded. Review round 1 showed that choosing it from the bounded position stops a tab
  more than 32 px wider than the end tab from ever passing it.
- No ADR: a bound on an existing interaction, local to one module.

## Evidence

- Root cause reproduced by a test: `npx e2e run tests/navigation.e2e.ts --grep "past either end"`
  on c9db7895 (test only) failed with `expected -52 to be greater than or equal to 31.5` (the first
  tab dragged to the window's edge was drawn at x = -52, past the row's start at 47.5); after the
  fix (f02a14fb) it passed. Undoing only the bounded settle start in `end` failed the BND-003
  assertion (released at -52).
- Review round 1 (major, own code): choosing the slot from the bounded position kept a tab more
  than 32 px wider than an end tab from passing it. Test 6b04f241 failed with
  `expected ["product","sandbox-3-with-a-much-longer-name","studio"] to equ…` (the wide tab stayed
  second); fix 31b4ada9 (slot from the pointer, only the drawing bounded) passes.
- Review round 2 approved 31b4ada9; the reviewer's mutants (slot from the bounded position, no
  drawn bound, settle from the pointer) each failed the new test. Nit fixed after it: the
  module's header comment rewrapped to the file's width (and one long line split), comments
  and layout only; the sandbox tab tests pass on the final head. Design note left as the spec intends
  (BND-004): a much wider tab pinned at an edge covers the end tab for the further half-width of
  pointer travel before the order flips.
- The SPEC-336 drag test asserted that sandbox-3 followed the pointer exactly to `first.x + 4`,
  which puts its left edge past the row's start; it now asserts the bound there (fails on the old
  code too: about 57 px past).
- `npx e2e run tests/navigation.e2e.ts tests/notes.e2e.ts tests/editor.e2e.ts` on 31b4ada9:
  82 passed (reviewer's run; the Notes and Editor drags are unchanged inside their strips).
- `scripts/dev ci` on 31b4ada9: exit 0, repository contracts passed, 2794 tests run, 2794 passed,
  16 skipped.
- `npx e2e run` (whole suite) on 31b4ada9: exit 0, 69 files passed, 1 skipped; 776 tests passed,
  1 skipped.
- `yulius security-scan`: **not run** (exit 2): Vidoc refused the key (HTTP 401 "Missing or
  invalid API key"); `tabdrag.js` and `navigation.e2e.ts` were not scanned (unrun, never passed;
  ADR-0133, ADR-0157). The pull request's CI `security` job is the live scan.
- Not tested: a real trackpad or pen by hand (the e2e drives Chromium's mouse), Firefox and
  Safari, and the bound on the Notes and Editor strips beyond their existing drag tests (same
  module, same code path).
