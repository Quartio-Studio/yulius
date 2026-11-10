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

## Requirements

- **TXT-001 Long words wrap.** Text in the app wraps a word that does not fit its line, at any
  character if it has to, so neither the text nor the flex or grid item holding it is wider than
  its box. Text meant to stay on one line keeps doing so: `white-space: nowrap` with an ellipsis,
  code and terminal views, and scrolling areas are unchanged.
- **TXT-002 The board card.** On the Git page, a board card whose label holds a long URL keeps the
  whole label inside the card at 1440, 768 and 390 px, the status icon beside its first line.
- **TXT-003 A guard for the class.** An e2e sweep gives the mock's tasks a long URL in a
  `needs_you` action and a long unbroken word in a title, opens the pages that show tasks at 1440,
  768 and 390 px, and fails when any visible text runs out of the nearest box drawn around it
  (border on both sides or a background). Text an ancestor clips or scrolls first is intended and
  does not count.

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] A sweep fails on text that runs out of its box on the pages that show tasks
- [ ] Long words wrap inside their box everywhere, by one base rule
- [ ] The board card's icon stays on its first line and the New task hint fits a narrow sidebar
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- The screenshot's case: the sweep fails on the Git page before the fix and passes after it.
- The sweep passes on every swept page at 1440, 768 and 390 px.
- The existing e2e suite passes (layouts unchanged where text fits).

## Test plan

- `e2e/tests/text-fits.e2e.ts`: the sweep (TXT-003) over Git, Tasks, Home, the task view, the task
  overview and the Dashboard, plus a check that the board card's icon sits on the label's first
  line (TXT-002). It fails on the current code (test first).
- Mutation check: without the base rule the sweep fails again.
- Full e2e suite and `scripts/dev ci` (the web assets are embedded in the server).

## Decisions

- To be recorded once the sweep has measured the current state.
