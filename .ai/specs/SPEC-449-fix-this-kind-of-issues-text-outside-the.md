---
yulius-task: e05d3bd9-f5b4-4ecc-afbc-028834951cc6
status: in-progress
---

# SPEC-449: Text stays inside its box

Status: in-progress
Date: 2026-11-10
Owner: Yulius task #431
Scope authorization: task #431 (`/goal`, with a screenshot of the Git page's board: the card of
task #145 in "Being built" shows "Needs you: Add TypeSafe API credits (or turn on auto-reload) at
https://console.typesafe.ai/settings/billing, then tell task #145", and the URL runs past the
card's right border), "fix this kind of issues (text outside the box)". Extends SPEC-001 UI-046
(layouts hold at every width) and follows SPEC-344 (a chip ran out of its card).

## Goal

No text runs out of the box drawn around it (a card, a column, a chip, a row with a border or a
background), at any window width, whatever words it holds: a long URL, path or identifier wraps
inside the box. The class of defect is caught by a test, not one screenshot at a time.

## Context

- **Symptom.** The board card's status line (`.dl-card-line` in `pages/git.js`) shows the task's
  label, here `Needs you: <action>` (`status.js` `taskInfoOf`). A URL in the action is one word
  wider than the card and is drawn past its border.
- **Root cause.** A word breaks only at spaces unless `overflow-wrap` allows more, and the
  stylesheet (`styles.css`) sets no `overflow-wrap` for the page. A word longer than the line
  then overflows. In a flex or grid row the text's item is also never narrower than its longest
  word (`min-width: auto` is the min-content width), so the row itself grows past the box.
  `.dl-card-line` is such a row (`display: flex`, the label an anonymous flex item).
- **Why it keeps coming back.** Each case was fixed where it was reported: about forty rules in
  `styles.css` add `overflow-wrap: anywhere` to one class (`.dl-card-title .dl-task`, `.chips >
  .badge` from SPEC-344, `.notif-detail`, …). Any new element showing user or agent text starts
  without it. The e2e layout checks (`responsive.e2e.ts`) catch overlapping controls and a page
  that scrolls sideways, not text that leaves its own box inside a page that still fits, and the
  mock's texts are short words.
- **What the sweep found.** Besides the board card (all three widths), the task overview's Needs
  you line made the page scroll sideways by 33 px at 390 px, and the New task button's shortcut
  hint (`Ctrl+Shift+O`, a `nowrap` button) ran 25 px out of the button in the 200 px sidebar used
  below 1024 px. The sidebar can also be dragged to 200 px at any width (UI-044).
- **Review round 1 (what the first fix broke).** The first fix set `overflow-wrap: anywhere` on
  `body`. It lowers every min-content width, so in a tight row short labels broke mid-word: the
  200 px sidebar's group names ("Done" over four lines, out of its 32 px head), the pipeline
  filters, the automation runs table. The sweep, with the word check added, fails 18 of 21 tests
  on that version. Done's Archive all also ran 40 px out of its head at 768 px, before any change.

## Requirements

- **TXT-001 Long words wrap, short ones do not break.** A word longer than its box breaks inside
  it, and no element's min-content width changes, so a layout where text fits is unchanged and a
  word of at most 12 characters is never broken across lines. Free text that is a flex item and
  may hold a URL (the board card's line, the task overview's state) may shrink below its longest
  word. Text meant to stay on one line keeps doing so: `white-space: nowrap` with an ellipsis,
  code and terminal views, and scrolling areas are unchanged.
- **TXT-002 The board card.** On the Git page, a board card whose label holds a long URL keeps the
  whole label inside the card at 1440, 768 and 390 px, the status icon beside its first line.
- **TXT-003 A guard for the class.** An e2e sweep gives the mock's tasks a long URL in a
  `needs_you` action and a long unbroken word in a title, opens every page of the responsive suite
  at 1440, 768 and 390 px, and fails when any visible text runs out of the nearest box drawn
  around it (a visible border on both sides or a background), a box runs out of the next box
  around it, the page or the window scrolls sideways, or a word of at most 12 characters is broken
  across lines. Contents an ancestor clips or scrolls first are intended and do not count. Pages
  are measured once loaded (no skeleton) and after two animation frames; a page that shows the
  long text (Git, Tasks, the task view, the task overview) only once it does. The pages are those
  of `responsive.e2e.ts` plus the task view.
- **TXT-004 The New task button.** Its text stays inside it at every sidebar width: the shortcut
  hint gives way when the sidebar is too narrow and shows when it is wide; the button's tooltip
  still names the shortcut.
- **TXT-005 Archive all.** In a narrow task list (the 200 px sidebar) Done's Archive all keeps its
  icon inside the group head and its name for screen readers; a wide list shows its label.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] A sweep fails on text that runs out of its box on the pages that show tasks
- [x] Long words wrap inside their box everywhere, by one base rule
- [x] The board card's icon stays on its first line and the New task hint fits a narrow sidebar
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- The screenshot's case: the sweep fails on the Git page before the fix and passes after it.
- The sweep passes on every swept page at 1440, 768 and 390 px.
- The existing e2e suite passes (layouts unchanged where text fits).

## Test plan

- `e2e/tests/text-fits.e2e.ts`: the sweep (TXT-003) over the 17 pages of `responsive.e2e.ts` and
  the task view; a check that the board card's label is inside the card, wraps and has its icon
  centred on the first line's rect (TXT-002); the New task button at every sidebar width from 200
  to 296 px in the splitter's steps, and at its widest (TXT-004); Archive all at 768 and 1440 px
  (TXT-005). They fail on the old code (test first).
- Mutation checks: without the two `anywhere` rules the Git, task overview and board card tests
  fail; with `anywhere` on `body` (round 1's first fix) 18 of 21 fail.
- Full e2e suite and `scripts/dev ci` (the web assets are embedded in the server).

## Decisions

- **`break-word` as the base, `anywhere` only for free text in a flex row.** `body {
  overflow-wrap: break-word }` breaks a word longer than its box and changes no min-content width,
  so nothing that fits today moves. Only `anywhere` lets a flex item shrink below its URL, and on
  `body` it broke short labels everywhere (review round 1), so it is set on the two items that
  hold free text (`.dl-card-line`, `.gt-state`). See
  [ADR-0289](../../docs/decisions/0289-long-words-wrap-by-default-and-a-sweep-keeps-text-inside-its-box.md).
- **The sweep checks both directions.** Text past its box catches the reported class; a short
  word broken across lines catches the opposite (wrapping that squeezes a label). A box is the
  nearest ancestor with a visible border on both sides or a background, which is what a person
  sees as one; each box is also checked against the next one around it, up to the first that clips
  or scrolls (an ellipsis or a wide table is intended). Vertical overflow allows a glyph's
  half-leading over a tight line-height.
- **Container queries, not media queries, for the sidebar.** The sidebar can be dragged to 200 px
  at any window width, so the window's width does not say whether the New task hint (hidden under
  a 240 px sidebar top; it needs about 226 px) or the Archive all label (hidden under a 250 px
  task list) fits. The hidden Archive all label stays readable to screen readers, like `.sr-only`.
- **The existing per-class `overflow-wrap` rules stay.** Each is an element's own choice.
