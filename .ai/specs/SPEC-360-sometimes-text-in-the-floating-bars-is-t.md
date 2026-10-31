---
yulius-task: c41c3875-7567-4d53-b475-4465bb55578d
status: done
---

# SPEC-360: A chart's hover tip wraps its text and stays inside its plot

Status: implemented
Date: 2026-10-31
Owner: Yulius task #340
Scope authorization: task #340 ("sometimes text in the floating bars is too big, fix … add line
breaks or something so the text is fully visible"). Fixes #986. Builds on SPEC-137 (stages
waterfall) and SPEC-165 (one row per job).

## Goal

Every word of a chart's hover tip is visible: long figures wrap onto more lines, and the tip
stays inside its plot whichever edge the pointer is near, at desktop and phone widths.

## Context

**Symptom.** In a CI run's stages waterfall, hovering a job with many figures (tests, CPU time,
peak memory, bytes written, exit code) showed a tip several hundred px wide whose left part was
cut off at the panel's edge ("…r a job slot", "skipped · CPU 0.0 s · …" in the reported
screenshot). The native `title` tooltip beside it wrapped and was fine.

**Root cause.** `.chart-tip` (`crates/yulius-server/web/assets/styles.css`) is
`white-space: nowrap` with no `max-width`, so it is as wide as its longest line. `show()` in
`waterfall()` (`assets/stages.js`) centres it on the pointer (`translate(-50%, …)`) and clamps
its `left` to `[80, width - 80]`, a bound that only fits a tip at most 160 px wide. A wider tip
runs past the plot by half its width minus 80 px. The Dashboard's daily chart (`.db-tip`) uses
the same clamp.

**Why the tests missed it.** The e2e suite checked the tip's text (`toContainText`), never its box,
and hovered the row's centre, where a wide plot leaves room on both sides.

**Reproduced.** e2e mock, run `ci_s2`, 1280 px, pointer 4 px into the `quality` job row: the tip
extends 40 px left of the plot.

## Requirements

- STT-001: A chart tip is never wider than its plot nor than 360 px; longer text wraps between
  words (a word longer than a line breaks anywhere).
- STT-002: The tip's left edge is clamped by its measured width, so all of it is inside its plot.
- STT-003: Both chart tips (the waterfall's and the daily chart's) use the same placement.
- STT-004: The waterfall tip breaks its figures list between figures ("wrote under 1 MiB" stays
  whole), each figure an inline block that still wraps inside itself only when wider than the tip.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- At 1280 px and 390 px, the tip of the `quality` job hovered at the row's left and right edge
  is 0 px past the plot and the window, and no part of it scrolls horizontally (text cut).
- The daily chart's tip on its first and last day, at 1280 px and 390 px, is 0 px past its chart
  and the window (it ran 7 px past at 1280 px before).
- The waterfall's first row (Queue), at the plot's top, keeps its tip inside the plot vertically too.
- The failed job's figures (the reported ones, now in the mock's run `ci_s2`) wrap onto more than
  one line at both widths, and every figure lies on one line.
- The existing waterfall and Pipeline e2e tests still pass.

## Test plan

- e2e `pipeline.e2e.ts`: "the waterfall's hover tip wraps its figures and stays inside the plot
  at every edge" (fails before the fix: 40 px past the plot at 1280 px) and "the daily chart's
  hover tip stays inside its chart on the first and the last day" (fails before: 7 px past). Each
  hover starts with the tip hidden, so the tip measured is the one just placed.
- The whole `pipeline.e2e.ts` and `responsive.e2e.ts` suites; then the full e2e suite and
  `scripts/dev ci` through Yulius CI.

## Decisions

- **Wrap with a capped width, not truncation.** The person asked for line breaks so the text is
  fully visible; an ellipsis would hide the figures the tip exists for. `width: max-content`
  with `max-width: min(360px, 100%)` keeps a short tip on one line, wraps a long one, and stops
  an absolutely placed tip from shrinking as it nears the right edge. 360 px keeps a readable
  line length (about 50 characters at the small size).
- **Clamp by the measured width** (`getBoundingClientRect().width`, unrounded, after the content is
  set; `Math.min` with the plot's width stays as a guard although `max-width` already caps it) instead of a fixed
  80 px, in one `placeTip` helper used by both charts. No ADR: a local layout fix that changes no
  contract.
- **Break between figures.** Wrapping words alone split "under 1 / MiB" across lines; each
  figure is an inline block, so a line ends at a " · " separator. Each dot ends its figure's block
  (a no-break space joins them), so a line breaks after a dot, never before one, by construction:
  a no-break space between two inline blocks is not guaranteed to stop a break (review round 2).
  The e2e test counts dots within an em of a line start as a guard; it could not be made to fail
  first: Chromium started no line with a dot for this content at any width from 320 to 1400 px,
  with a plain " · ", a no-break " · " or the current separator (probe in Evidence). `figureParts()` returns the figures as a list (it replaces `figureText()`,
  which joined them): the row's `title` and the table join them, the tip keeps them apart.
- **The mock's failed job carries the reported figures** (tests, CPU, peak memory, bytes
  written, exit 101) so the e2e test hovers a tip that must wrap; no other test reads them.
- **The tip stays above the pointer** (review round 1 asked whether a tall tip rises above the
  plot). The test measures the top row's tip against the plot's top and bottom and it fits: the
  rows near the top (queue, run, prepare) carry short figures, and the legend and axis sit above
  them. A flip below the pointer would be untested code for a case no timeline produces, so it is
  left out; the test guards the bound.
- **What else the cause touches.** Only these two tips use `.chart-tip`; other floating bars
  (`.tg-node`, menus, toasts) have their own rules and are not changed.

## Evidence

- Reproduced on dev: e2e mock run `ci_s2`, 1280 px, pointer 4 px into the `quality` row: tip
  40 px past the plot. Issue #986.
- Regression tests (`e2e/tests/pipeline.e2e.ts`) fail without the fix: the waterfall test at
  e7f63fdb against dev's code (40 px past); the daily chart test against dev's `stages.js` and
  `styles.css` (7 px past at 1280 px); the whole-figure assertion before 12cfdcec (`[]` figures).
- Mutation checks, each failing the waterfall test: `white-space: nowrap` restored (text cut),
  the fixed 80 px clamp restored (40 px past), dev's `stages.js` and `styles.css` (40 px past).
- Screenshots at 1280 and 390 px (local, mock `ci_s2`, failed job hovered at its row's left edge):
  the tip is inside the plot on three lines, each figure whole.
- Dot probe (local Playwright, widths 320-1400 px step 1, failed job's tip): no line starts with a
  dot (within an em of the edge) for any of the three separators; the probe with no threshold
  finds dots at every width (sanity check).
- At 993e74d2: full e2e `npx e2e run`: 69 files passed, 1 skipped; 782 tests passed, 1 skipped.
  `python3 scripts/bootstrap.py && scripts/dev ci`: exit 0 (nextest 2888 passed, 16 skipped).
- After 993e74d2 only the test's dot threshold and this spec changed: `pipeline.e2e.ts` and
  `responsive.e2e.ts` rerun: 41 passed; Yulius CI runs every job on the head.
- Security: `yulius security-scan` at 55124d48 did not run: Vidoc refused the key (HTTP 401,
  exit 2). Unrun, not passed; the CI `security` job reports its own result.
- Reviews in fresh sessions: round 1 (7beed878) request changes, all findings fixed test-first
  (daily chart test, hover from off the chart, vertical bounds tested, figures as a list,
  unrounded width); round 2 (55124d48) approve with minors, fixed (dot inside its figure, spec
  wording); round 3 (993e74d2) approve with one minor, fixed (em threshold, spec wording).
