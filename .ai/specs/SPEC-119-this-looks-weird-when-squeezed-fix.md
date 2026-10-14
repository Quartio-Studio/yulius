---
yulius-task: 251c8280-fdce-43f8-b318-8dc2dcf93b63
status: in-progress
---

# SPEC-119: The new-task composer's controls give way in rows that read as one layout

Status: in progress
Date: 2026-10-14
Owner: Yulius task #104
Scope authorization: task #104 ("This looks weird when squeezed. Fix.", with a screenshot of the
new-task composer in a narrow column). Fixes SPEC-001 UI-021 together with UI-046 for the composer.
Issue: #442.

## Goal

Squeezed, the new-task composer keeps its controls tidy. The choices take the first rows. Preview,
model, effort and Start take a full-width last row that starts at the box's left edge and ends with
Start at its right edge. No control leaves the box.

## Context

**Symptom.** In the screenshot the prompt box is about 655 px wide. Auto mode, Auto workflow and
Start now sit on the first row. The eye (Preview), "Opus 5.5", "High" and the ↑ button sit on a
second row, pushed to the right edge, and the left half of that row is empty.

**Reproduction.** In mock mode (`?mock=1#/s/studio`), with default labels:

| Window | Prompt box | Second row starts at |
| --- | --- | --- |
| 900 px | 668 px | 338 px |
| 1440 px with the Shell panel open | 507 px | 177 px |

In a 1024 px window with a panel the box is 328 px, and Start ends 6 px past the box's border. In
a 390 px window "Start now" sits alone on row 2 and the right group on row 3, indented from the
chips.

**Root cause.** `styles.css` lays out `.composer-tools` as a wrapping flex row whose last item,
`.composer-right` (Preview, Model, Effort, Start), is one block with `margin-left: auto`. When the
row does not fit, that whole block wraps, and its auto margin keeps it at the right edge of an
otherwise empty row. Its chips cannot shrink, so at the narrowest widths the block is wider than
the box. Default labels need a box of about 742 px for one row (at most 760 px), so nearly any
squeeze shows this.

**Why the tests missed it.** `responsive.e2e.ts` checks pages for overlapping header, toolbar and
list controls, and the composer's controls are not in its selector. No test measured the composer
at a narrow width.

**What else the cause reaches.** Only `.composer-tools` and `.composer-right` are affected.
`.chip-btn` is shared with the automation editor's start chip (`.au-start`), so changes to it stay
scoped to `.composer-tools`.

## Requirements

- CMP-SQ-1: When the prompt box is at least 740 px wide, all the composer's controls sit on one
  row as before. Long chip labels truncate with an ellipsis instead of wrapping the row.
- CMP-SQ-2: When the prompt box is narrower than 740 px, Preview, Model, Effort and Start take a
  row of their own, the last one. It starts at the same left edge as the first row, and Start ends
  it at the right. The choices (mode, workflow, run after) wrap above it from the left.
- CMP-SQ-3: No control extends past the prompt box, and no two controls overlap. This holds down
  to a 390 px window and a 1024 px window with a tool panel. When space is short, the model and
  effort chips truncate their labels.

## Plan

- [x] Reproduce: measure the composer's controls at narrow windows and beside a tool panel.
- [x] Regression test in `e2e/tests/responsive.e2e.ts` that fails on the current CSS.
- [x] Make the prompt box a size container and lay out its controls in two deliberate rows below
  740 px.
- [ ] Gates: the e2e suite and `scripts/dev ci`. Independent review.

## Acceptance criteria

- The new e2e test passes. With the old CSS it fails: `lastRowAligned` is false in a 900 px window.
- The composer suites (`new-task`, `drafts`, `intake`, `menus`, `prepared`, `supertask`,
  `onboarding`, `phone`) still pass.

## Test plan

`e2e/tests/responsive.e2e.ts`: "the new-task composer squeezed keeps its controls in the box, the
last row starting at the left". It covers the following (CMP-SQ-1 to 3):
- One row in a wide window.
- At 900, 768, 600 and 390 px windows, and beside the Shell panel at 1920, 1440, 1280 and 1024 px:
  - every control is inside the box,
  - no two controls overlap,
  - the last row starts at the first row's left edge,
  - Start is on the last row, at its right end.

## Decisions

- **Query the prompt box's own width (`@container composer`).** UI-046 already uses this approach
  for pages. The window's width would miss a tool panel, and so would the page's width, because the
  composer also centres itself within a 760 px card. No new ADR: this applies the existing
  decision to one more component.
- **Use a fixed 740 px breakpoint rather than detecting the wrap in JavaScript.** CSS cannot tell
  when a flex row wraps. A ResizeObserver would work, but it adds code and a frame of the wrong
  layout. Above 740 px the chips shrink with an ellipsis instead of wrapping, so long labels cannot
  bring the lopsided row back. Default labels fill one row at about 742 px, so the box only shows
  one row at its full width.
- **Keep Start on the last row, at its right end.** The send button stays where a chat input puts
  it, and model and effort sit next to the choices they qualify. Collapsing chips to icons was
  rejected: mode and workflow share the same lightning icon.

## Evidence

- Old CSS (`61af0105`, the test alone):
  `npx e2e run tests/responsive.e2e.ts --grep "new-task composer squeezed"` fails with
  `expected {"outside":[],"overlaps":0,"lastRowAligned":false,…}` (900 px window).
- With the fix (`8fb9acc2`), the same test passes.
- Screenshots of the prompt box (Playwright, dark theme):
  - 1440 px: one row as before.
  - 900 px: the chips on row 1, then Preview, "Default model" and "Default effort" from the left
    edge, with Start at the right.
  - 1024 px with the Shell panel: the chips wrap from the left, the last row is the same, the
    model and effort labels truncate, and Start stays inside the box.
- Full e2e suite (`npx e2e run`): 345 passed, 1 skipped, 1 failed. The failure was in
  `pulls.e2e.ts`, "announcements within the gap make one read when it has passed", which is
  unrelated: a wall-clock 1.5 s window under load. It passed 3 times out of 3 when run alone and is
  filed as #444.
- After merging `origin/dev` (`17b2fefa`): `responsive`, `new-task`, `places`, `menus`, `drafts`,
  `phone`, `intake`, `onboarding`, `prepared` and `supertask` give 114 passed.
- `scripts/dev ci`: exit 0, with nextest at 1604 passed and 16 skipped.
