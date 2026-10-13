---
yulius-task: 9ad72835-d7ee-4604-8391-2d7f6b65b968
status: done
---

# SPEC-073: A long task state no longer pushes the step count under the progress bar

Status: implemented
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

- STR-1: The state takes at most 40% of the strip. Beyond that it truncates with an ellipsis, and
  its full text stays in the pill's tooltip, which `title` already sets. A state that fits keeps
  its words whole: the step name, then the title, give way first.
- STR-4: While the step count shows (a strip over 420 px), the label never gets less than 8 rem,
  room for the chevron, "Step NN/NN" and a little of the title. Below that, the state pill gives
  its words up first, down to its icon. Where the count hides (420 px and phones), the floor
  goes too.
- STR-2: Nothing in the strip's label paints outside its button. If the label is ever too long
  for its room, it is clipped inside the toggle and never drawn over the bar.
- STR-3: On mock task #14, no strip parts overlap and "Step N/M" ends inside its button at 1440,
  1024 and 768 px, and beside an open tool panel. Beside the Shell panel, tasks #13 ("Done · PR
  #520" plus its chip), #14 and #7 keep the count inside at windows from 1300 to 1440 px. That
  sweep reaches the 421 to 500 px strip band. On task #12 at 768 px, the short
  state ("Working") stays whole.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Regression test: a long task state no longer pushes the step count under the bar
- [x] Fix: the state text gives way and the strip label never paints past its button
- [x] Review, gates and delivery to dev
- [x] The label keeps room for its step count beside a tool panel (421-560 px strips)
- [x] Final review round, gates and merge into dev

## Acceptance criteria

- The new `responsive.e2e.ts` long-state case fails on `dev` with a `step-count × step-progress`
  overlap and passes with the fix. The short-state case fails on the first fix attempt
  (`stateWhole: false`) and passes with the final one.
- The rest of `responsive.e2e.ts` and the full e2e suite stay green.

## Test plan

- e2e (`responsive.e2e.ts`): open task #14 at 1440, 1024 and 768 px, and at 1440 px with the
  Shell panel open. Wait on `layoutProblems()` until there are no overlaps and no sideways
  scroll, and on the step count ending inside the toggle. The overlap check trims boxes to
  clipping ancestors, so it cannot see a count the toggle clips. The test also asserts that the
  state is long, so it keeps covering the case.
- e2e: on task #12 at 768 px, the short state is whole (`scrollWidth <= clientWidth`) and the
  count is inside the toggle.
- Full e2e suite and `scripts/dev ci` as the gates.

## Decisions

- **The state is capped at 40% of the strip; the label keeps the rest.** The toggle stays
  `flex: 1; min-width: 0`, so it takes what the other parts leave, as before. `.strip-status`
  gets `max-width: 40%` and `min-width: 0`, and `.strip-state` gets an ellipsis. A long state
  can no longer starve the label, and a state that fits is never cut. The state's full text is
  one hover away, and its icon and colour still carry the meaning.
  Alternatives:
  - The first attempt made the toggle `flex: 1 1 auto` and the pill shrinkable. The review
    found that flex shrinks in proportion to each part's basis, so even "Working" truncated
    while the step name still showed. That broke UI-046's order: the step name first.
  - A floor on the toggle (`min-width: 12rem`) plus a tiny shrink factor on the pill does keep
    that order. But the floor overflows the phone strip, where the count and state are already
    hidden, so it would need another breakpoint.
  - Lower container-query breakpoints are guesses that the next long label breaks again.
  This is a local CSS rule, so it gets no ADR.
- **The label has a floor of 8 rem while the count shows (review round 2).** The cap alone left a
  band where a short state still squeezed the label: a 421 to 560 px strip beside a tool panel,
  where the bar, the state and the pull-request chip leave less room than the count needs.
  Round 2 measured "Ste" on task #13 at a 459 px strip, the reported screenshot's case.
  With the floor:
  - The toggle (`flex: 1`, basis 0) keeps at least 8 rem.
  - The state pill, which can shrink (`min-width: 0`), gives its words up first, down to its icon.
  - The arithmetic holds at the narrowest strip that still shows the count, 421 px: 60 px bar,
    34 px icon-only pill, about 55 px PR chip, 28 px menu, 40 px gaps, 12 px padding and 128 px
    floor, 357 px in all.
  - The 420 px container query and the phone media query set `min-width: 0` again, because the
    count is hidden there.
  Alternatives from the review:
  - A narrower cap in the band leaves #13 untouched, since its room goes to the PR chip.
  - Hiding the PR chip or the reset text loses information that has room elsewhere.
- **The toggle clips (`overflow: hidden`) as a backstop.** The step count stays `flex: none` so it
  reads whole. If it ever still runs out of room, the toggle clips it instead of letting it paint
  over the bar.

## Evidence

- Reproduced on `dev` (`?mock=1#/s/studio/t/14`, 768 px). The state pill is 290 px and the toggle
  46 px. "Step 7/7" ends at x=297 and the bar starts at x=262. A screenshot of the strip matches the
  reported one: no title, and the bar through "Step 7/7".
- `npx e2e run tests/responsive.e2e.ts` before the fix (commit 6e402a5):
  `last: expected {"overlaps":["span.step-count \"Step 7/7\" × div.step-progress …"]}`, 1 failed |
  16 passed.
- With the first fix attempt, the new short-state case failed: `{"countInside":true,"stateWhole":false}`.
- With the final fix, every strip case in `responsive.e2e.ts` passes: long state, short state, and
  task #12 beside a tool panel. The only failures were page-heading timeouts on first load at load
  average 37 to 47 on 12 CPUs, on pages unrelated to the strip.
- Screenshots at 1440 and 768 px: the state reads in full at 1440 px and ends in "…" at 768 px, with
  "Step 7/7" whole beside the bar.
- `scripts/dev ci` exited 0: repository checks, check, fmt, clippy, and nextest with 1324 tests
  run, 1324 passed.
- The full e2e suite could not give a signal locally. About 30 tests across unrelated suites
  (editor, menus, agent panel) timed out at load 45 on 12 CPUs. Yulius CI's e2e job at the
  pull-request head is the authority.
- Independent review (a fresh session, `om-code-review`). Round 1 asked for changes: the
  proportional shrink truncated short states, and the overlap check was blind to a clipped
  count. Both are fixed in this change, each with a test.
- Review round 2 asked for changes at 8c6ad93: the count was still clipped in a 421 to 490 px
  strip beside a panel. The new narrow-band test failed at 8c6ad93 (`last: expected false to be
  true` on `countInside`). With the floor (465d1e0), `responsive.e2e.ts` and `phone.e2e.ts` pass
  21 of 21.
- A playwright sweep covered mock tasks #1 to #14, windows from 390 to 1440 px in 10 px steps,
  with and without the Shell panel. It reported `NO PROBLEMS`: the count never ends outside its
  button, the strip's last part never passes its edge, and nothing scrolls sideways.
