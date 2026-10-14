---
yulius-task: 31817ae5-f7fc-4a78-a199-a43975e8281b
status: done
---

# SPEC-115: The Git page gives way to the page's width, not the window's

Status: implemented
Date: 2026-10-14
Owner: Yulius task #100
Scope authorization: task #100 ("it get's broken when squeezed. fix", with two screenshots of
the Git page beside the Editor panel). Fixes SPEC-001 UI-046 (AC-UI-41) for the Git page.

## Goal

The Git page stays readable when a tool panel squeezes it. Its summary cards and its board
columns stack instead of shrinking until task titles break letter by letter.

## Context

**Symptom.** In the screenshots the Editor panel is open next to the Git page. The board's four
columns ("Being built", "In review", "On dev, not live", "Live") stay side by side, each a few
letters wide. Task #99's title breaks one or two letters per line ("Val / ue / in / pa / ss").
The two summary cards ("Live in production", "Waiting to go live") wrap their headings word by
word.

**Reproduction.** Open `?mock=1#/s/studio/git` in a 1920 px window with the Editor panel open.
The narrowest board column is 95 px wide.

**Root cause.** `styles.css` lays out `.dl-board` (columns) and `.dl-strip` (summary cards) with
`@media (max-width: 1023px)` and `@media (max-width: 767px)`. Those queries read the window's
width. A tool panel shrinks the page, not the window, so a wide window keeps four columns in a
page a few hundred pixels wide. Since UI-046, `.page` is a size container (`container: page`),
and other pages (`.item-card`, sessions) already query it. The Git page was never moved over.

**Why the tests missed it.** `responsive.e2e.ts` checks the Git page only at 768 and 390 px
windows with no panel open, where the window queries happen to match. Only the Tasks page and a
task page were checked beside a panel.

**What else the cause reaches.** In the same `@media (max-width: 767px)` block, the Blocked rows
(`.dl-need`) and the environment rows (`.dl-env`) wrap their buttons onto their own row. That
block also keyed on the window, so beside a panel those rows did not wrap either. They move to
the page query too. The button heights stay on the window query, because they size touch
targets on phones. No other `@media` width rule in `styles.css` lays out a `.dl-` element.

## Requirements

- GIT-SQ-1: The board shows its columns side by side only while the page is wider than 900 px.
  From 561 to 900 px it shows two columns per row, and at 560 px and below one column.
- GIT-SQ-2: The two summary cards sit side by side above 560 px of page width and stack at
  560 px and below.
- GIT-SQ-3: At 560 px of page width and below, a Blocked row's or environment row's buttons wrap
  onto their own row, beside a panel as on a phone.
- GIT-SQ-4: With the Editor or the Shell panel open in 1920, 1440 and 1024 px windows, no board
  column or summary card is narrower than 180 px. Controls do not overlap and the page does not
  scroll sideways. Without a panel, a 1440 px window still shows the board in one row.

## Plan

- [x] Reproduce: a mock Git page beside the Editor panel, with the narrowest column measured.
- [x] Regression test in `e2e/tests/responsive.e2e.ts` that fails on the current CSS (95 px).
- [x] Move the Git page's layout breakpoints from `@media` to `@container page`.
- [x] Gates: the e2e suite and `scripts/dev ci`. Independent review.

## Acceptance criteria

- The new e2e test passes. With the old CSS it fails ("expected 95 to be greater than or equal
  to 180").
- The responsive checks at 768 and 390 px still pass for the Git page.

## Test plan

`e2e/tests/responsive.e2e.ts`: "the Git page beside a tool panel keeps its board columns and
summary cards wide enough to read" (GIT-SQ-1, 2 and 4). The existing 768 and 390 px Git page
check covers phones (GIT-SQ-3 and the button heights).

## Decisions

- Query the page container, not the window, as UI-046 already set for lists. That needs no new
  ADR: it applies an existing decision to one more page.
- Fixed breakpoints rather than `repeat(auto-fit, minmax(…))`. The board has at most four
  columns, and auto-fit would leave a 3 + 1 split that reads as a broken board.
- Windows from 561 to 767 px (a tablet, a large phone) now follow the page's width too. A page
  of that size shows two board columns of about 300 px, the summary cards side by side, and
  Blocked rows with their buttons inline, where the window query stacked everything. That is
  deliberate: the 180 px floor holds, and the same page width looks the same with or without a
  panel.
- 900 px: four columns of a 900 px page (32 px side padding, 12 px gaps) are about 200 px each,
  enough for "On dev, not live" on two lines. 560 px matches the existing page breakpoint for list
  rows (UX2-05).

## Evidence

- Old CSS (`eec55e98`): `npx e2e run tests/responsive.e2e.ts --grep "Git page beside"` fails
  with "expected 95 to be greater than or equal to 180" (Editor panel, 1920 px window).
- With the fix: `npx e2e run tests/responsive.e2e.ts` gives 21 passed. `tests/git.e2e.ts`,
  `tests/phone.e2e.ts` and `tests/loading.e2e.ts` pass too (31 passed across the four files).
- Screenshot of the mock Git page beside the Editor in a 1440 px window: the summary cards are
  stacked, the board is one column, and task titles read whole.
- `scripts/dev ci`: exit 0, with nextest at 1604 passed and 16 skipped.
- Independent review (fresh session, `om-code-review`): APPROVE. Its minor findings are fixed in
  `82b643bc`. That commit guards against an empty board, adds overlap checks for Blocked,
  environment and board rows, and records the 561 to 767 px change as a decision. The pre-existing
  dead `.dl-release .btn` rule is left as it was.
