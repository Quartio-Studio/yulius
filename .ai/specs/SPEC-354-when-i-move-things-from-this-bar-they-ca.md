---
yulius-task: c3ac7da8-9749-4a0b-96fe-99f2e6075f96
status: in-progress
---

# SPEC-354: a dragged tab stays in its strip and bounces off its ends

Status: in progress
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
| BND-004 | The order chosen is unchanged: dragging far past an end puts the tab at that end. |
| BND-005 | The same bound applies to the sandbox, Notes and Editor tab strips (one shared module). |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] Failing e2e: a sandbox tab dragged past either end of the row stops at the border
- [ ] Shared tab drag bounds the dragged tab with an elastic overshoot
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

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
- No ADR: a bound on an existing interaction, local to one module.

## Evidence

(to be filled)
