---
yulius-task: 0be731e0-8df7-4122-b641-57f1b95712f3
status: in-progress
---

# SPEC-185: The watched transcript follows the bottom, and has no octopus

Status: in-progress
Date: 2026-10-16
Owner: Yulius task #170
Scope authorization: task #170: "this screen is bugged - "working" etc is bugged, it isn't
scrolled down automatically. And remove the octopus, it was an overkill" (with a screenshot of a
watched task, SPEC-179: working, "New messages" shown, the octopus at the top); then "also reduce
horizontal padding in this view".
Fixes #575.

## Goal

Someone watching another person's task (SPEC-179) sees its newest lines while the agent works: as
long as they have not scrolled up, the live transcript stays at its bottom whatever changes its
height. The pixel octopus along the transcript's top goes, with its lane, and the transcript sits
closer to the view's sides.

## Context

- **Symptom.** A watcher of a working task sees the transcript stop above its bottom with
  "New messages", and it no longer scrolls down by itself (the screenshot; #575).
- **Root cause.** `watchview.js` scrolls to the bottom only in `apply()`, when a read brings
  entries, and decides whether the reader follows by measuring the distance to the bottom at that
  moment (`atBottom()`, 32 px slack). Heights change between reads with no scroll: the
  `✻ Working…` line shown under the last line whenever the agent works (12 px margin and a line:
  exactly 32 px at 20 px lines, more with taller ones), the control card above (its notice comes
  and goes with the task's state), the window and side panels, fonts, and session blocks laid out
  late (`content-visibility: auto`, ADR-0143). The view is left above the bottom; the next read
  measures more than the slack, takes the reader for one who scrolled up, shows "New messages" and
  stops following for good. Chromium's scroll anchoring sometimes moves the view along (it hid the
  defect from the SPEC-179 tests); Safari has no scroll anchoring.
- **Why the tests missed it.** AC-WAT-3 appends entries while at the bottom and scrolls up by
  script; no test changed a height between reads, and in Chromium the anchoring or the control
  card shrinking as the activity became `working` made up for the Working line.
- **The octopus** (SPEC-179 WAT-006, ADR-0145): `octopus.js`, the `.watch-lane` sticky lane (80 px
  over the transcript's top), the `--octo-*` colour and duration tokens, its CSS and four e2e tests.
  The person who asked for it now finds it overkill.
- A watcher is also told "Type it in the terminal below" when the agent waits; that is #576, not
  this change.

## Requirements

- **FOL-001 Following is a state.** The watched view follows the bottom from its first read until
  the reader scrolls up; it follows again once they are back at the bottom (by scrolling or with
  "New messages"). Only the reader's scrolling up stops it: a height that changes below them does
  not.
- **FOL-002 Heights do not lose the bottom.** While following, any change of the transcript's
  height (the Working line, entries laid out late, a result reprinted in its call) or of the view's
  (the control card, the window, panels) keeps the view at the bottom, without a read.
- **FOL-003 New lines.** While following, a read's new entries are shown at the bottom with no
  "New messages"; scrolled up, the view stays where the reader is and "New messages" appears (as
  AC-WAT-3).
- **OCT-001 No octopus.** A watched task shows no octopus and no lane: the transcript starts at the
  top of its well. `octopus.js`, its preload, the `--octo-*` tokens and their CSS are removed.
- **PAD-001 Less padding.** The watched transcript is inset 16 px at its sides, as the control card
  above it (its left edge lines up with the card's), not the 32 px of a past session's page; at
  phone width it keeps 12 px. Past sessions are unchanged.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [x] The watched transcript has no octopus, its docs and tests updated
- [x] The watched transcript is inset 16 px, as the card above it
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Merged into dev on green CI

## Acceptance criteria

- **AC-FOL-1.** e2e (scroll anchoring off on the scroller, as in Safari, lines 24 px): at the
  bottom of an idle task, the Working line appearing keeps the view at the bottom; a shorter view
  keeps it at the bottom; the next entries are followed with no "New messages". Fails before the
  fix (36 px above the bottom).
- **AC-FOL-3.** e2e (review): at the bottom, the Working line hiding (the end pulls the view up)
  keeps it following; scrolled up and back down by hand, it follows again; a reader 50 px up whom
  the Working line going leaves within the slack is followed by the next lines. The last fails
  without the observer taking up a reader at the bottom (54 px above it).
- **AC-FOL-2.** The existing AC-WAT-3 tests pass: scrolled up, new entries do not move the view
  and "New messages" jumps down; a late result alone shows no button.
- **AC-OCT-1.** e2e: a watched task has no `.octo` and no `.watch-lane`; its first line is at the
  top of the transcript's well. Past sessions keep the spider.
- **AC-PAD-1.** e2e: the watched transcript's side padding is 16 px and its left edge is 16 px left
  of the card's; 12 px at phone width; a past session keeps 32 px. Fails before the change (32 px).

## Test plan

- `e2e/tests/watching.e2e.ts`: the AC-FOL-1 regression test (written first, seen failing), the
  AC-OCT-1 test replacing the octopus tests, the AC-PAD-1 test; the rest of the file unchanged.
- Gates: `scripts/dev ci`, `scripts/dev security`, the full e2e suite.

## Decisions

- **Follow by observing sizes (ADR-0149).** A `following` flag set from the reader's scrolling
  (back at the bottom: on; moved up away from it: off), and one `ResizeObserver` on the scroller
  and its content that puts a following view back at the bottom. Alternatives: a larger slack (any
  fixed slack loses to a taller change: a three-line card, a big diff laid out late); scrolling
  down on each task event (misses resizes, fonts and late layout); `overflow-anchor` tricks
  (Safari has none). Only scrolling up stops following, so a content change that the browser
  answers with a scroll (anchoring moving the view down, the end clamping it up) never does.
- **The octopus goes whole (ADR-0149 amends ADR-0145).** Removing the lane with it gives the 80 px
  back to the transcript. The spider and cockroach of past sessions stay (not asked).
- **16 px, as the card.** The control card above the transcript sits 16 px from the view's edges;
  the transcript takes the same inset so both start on one line. Past sessions keep the Sessions
  page's 32 px (the request was about this view).

## Evidence

(Filled at the end.)
