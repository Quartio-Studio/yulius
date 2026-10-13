---
yulius-task: 9ad72835-d7ee-4604-8391-2d7f6b65b968
status: in-progress
---

# SPEC-073: A long task state no longer pushes the step count under the progress bar

Status: in progress
Date: 2026-10-13
Owner: Yulius task #68
Scope authorization: task #68 ("this bar is broken, fix", with a screenshot of the task strip);
tracked as GitHub issue #341. Fixes SPEC-001 UI-046 (AC-UI-41) for strips with a long state.

## Goal

In a task's step strip, the progress bar never covers "Step N/M" (or anything else in the
label), whatever the strip shows on its right.

## Context

**Symptom.** The screenshot shows the strip's chevron, "Step 0/0" (the digits hidden by the bar)
and the progress bar drawn over the text, with the Done state on the right.

**Reproduction.** `?mock=1#/s/studio/t/14` at 768 px: the state pill reads "Working · 1
background task still running · N min" and is 290 px wide. The toggle (`.stepstrip-toggle`)
is left with 46 px. `.step-count` is `flex: none`, so it overflows the toggle by 15 px. It ends
at x=297, and the progress bar starts at x=262. Other long states do the same: "Done · PR #N
merged", "In review · PR #N · checks running", "Prompt held · your choice".

**Root cause.** In `.stepstrip-main`, the toggle (`flex: 1`, `min-width: 0`) is the only part
that gives way. The state pill (`.strip-status`) keeps its full text width, because a flex item
keeps its content size by default. The container queries from UI-046 hide the step name at
560 px and the count and state at 420 px. They key on the strip's width alone, not on what the
right side holds. So a long state leaves the toggle less room than its `flex: none` step count
needs, and the count, which nothing clips, paints into the progress bar next to it.

**Why the tests missed it.** `responsive.e2e.ts` checks the strip only on task #12, whose state is
short ("Working"). Its overlap check would have caught #14.

**What else the cause reaches.** Every part to the right of the toggle (chain chips, the pull
request chip, the state) has the same fixed-width behaviour. The state is the only one whose
text is unbounded. The chips are short and already hidden below 560 px.

## Requirements

- STR-1: The state text truncates with an ellipsis before the task title and step count lose
  their room. Its full text stays in the pill's tooltip, which `title` already sets.
- STR-2: Nothing in the strip's label paints outside its button. If the label is ever too long
  for its room, it is clipped inside the toggle and never drawn over the bar.
- STR-3: On mock task #14, no strip parts overlap at 1440, 1024 and 768 px, or beside an open
  tool panel.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Regression test: a long task state no longer pushes the step count under the bar
- [ ] Fix: the state text gives way and the strip label never paints past its button
- [ ] Review, gates and delivery to dev

## Acceptance criteria

- The new `responsive.e2e.ts` case fails on `dev` with a `step-count × step-progress`
  overlap and passes with the fix.
- The rest of `responsive.e2e.ts` and the full e2e suite stay green.

## Test plan

- e2e (`responsive.e2e.ts`): open task #14 at 1440, 1024 and 768 px, and at 1440 px with the
  Shell panel open. Wait on `layoutProblems()` until there are no overlaps and no sideways
  scroll. The test also asserts that the state is long, so it keeps covering the case.
- Full e2e suite and `scripts/dev ci` as the gates.

## Decisions

- **The state gives way, not the title.** UI-046 already says the title leads. The state's full
  text is one hover away, and its icon and colour still carry the meaning. Alternatives: lower
  container-query breakpoints, which are guesses that the next long label breaks again, or
  hiding the state text at a fixed width, which loses it on wide strips that have room. Neither
  holds for unbounded text. This is a local CSS rule, so it gets no ADR.
- **The toggle clips (`overflow: hidden`) as a backstop.** The step count stays `flex: none` so it
  reads whole. If it ever still runs out of room, the toggle clips it instead of letting it paint
  over the bar.
