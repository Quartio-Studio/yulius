# 0147. The step strip fits itself in the page, giving way whole parts by priority

## Status

Accepted. Specified in [SPEC-182](../../.ai/specs/SPEC-182-fix-348-331-407-449-the-step-strip-never.md)
(task #138, issue #348). It extends SPEC-073's strip rules (the state's 40% cap and the label's
8 rem floor), which stand.

## Context

A task's step strip is one flex row: the step label, the progress bar, the state, the chips
(Supertask, Auto or Goal, "After #N" or "When …", "Then #N"), the pull request chip and Task
actions. Chips keep their text width, so with enough of them the row's smallest width passed the
strip's: beside a tool panel at 1480–1560 px windows the menu slid under the panel, and in strips of
110–300 px (770–960 px windows beside the Shell) every part from the bar on passed the edge. Two
container queries (560 and 420 px) hid parts by the strip's width alone, but what fits depends on
what the strip holds: how many chips, how long the state, whether there is a pull request. That is
also how #341 came about.

## Decision

- `fitStrip()` in `taskview.js` runs after every paint of the strip and of its delivery chip, on
  every size change of the row (one `ResizeObserver`) and when a web font finishes loading
  (`document.fonts` `loadingdone`, which changes the parts' widths but not the row's); both are
  released with the view. It shows every
  part, then hides parts while the row's content is wider than the row, in this order: the chain
  chips (the last "Then #N" first, "After #N" or "When …" last among them), Auto or Goal,
  Supertask; then, with the state allowed to shrink again (SPEC-073), the progress bar and the pull
  request chip. While chips are decided the state keeps its words (`flex-shrink: 0` under
  `.strip-fitting`), so chips give way before them. The label, the state's icon (the live status)
  and Task actions always stay.
- A part gives way whole, through the class `strip-gone` (`display: none`): it leaves the tab order
  and the accessibility tree, and `hidden` stays the parts' own (the bar hides with no plan).
  Keyboard focus on a part that goes moves to the strip's label.
- The container queries stay for what depends on width alone (the step name, the count and the
  state's words); the 560 px query hides the chips' box itself (`.stepstrip .strip-extras`, which
  outranks the later base rule), so an empty box takes no gap.

## Alternatives

- More container query breakpoints: each guesses a width that depends on the strip's content, the
  cause of both #341 and #348. Rejected.
- A wrapping chip row clipped by `overflow: hidden`, so chips that wrap are cut away in CSS alone.
  The clipped links stay focusable and are read, and the order between the chips and the state's
  words cannot be set. Rejected.
- Shrinking chips with an ellipsis: the issue asks for whole chips, and "Then #1…" says nothing.
  Rejected.

## Consequences

- No strip part passes its edge at any width (`responsive.e2e.ts` sweeps 770–1920 px windows with
  Auto, After and two Then chips, with and without the Shell panel).
- A new part of the strip needs its place in the order, or it never gives way.
- Each fit forces a few synchronous layouts of the strip only; it runs on paints and resizes, not
  on every frame.
