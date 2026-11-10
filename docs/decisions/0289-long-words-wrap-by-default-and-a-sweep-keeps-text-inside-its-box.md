# 0289. Long words wrap by default, and a sweep keeps text inside its box

## Status

Accepted. Specified in [SPEC-449](../../.ai/specs/SPEC-449-fix-this-kind-of-issues-text-outside-the.md)
(task #431). Extends SPEC-001 UI-046 (layouts hold at every width). Follows SPEC-344, which fixed
one chip the same way.

## Context

A Git board card showed "Needs you: … at https://console.typesafe.ai/settings/billing, then tell
task #145", and the URL ran past the card's border. Two things combine here:

- A word breaks only at spaces unless `overflow-wrap` allows more, and the stylesheet set nothing
  for the page.
- In a flex or grid row, the item holding the text is never narrower than its longest word
  (`min-width: auto`, the min-content width).

About forty rules each add `overflow-wrap: anywhere` to one class, every one after a report like
this one, and the next new element started without one. The layout checks (`responsive.e2e.ts`)
look for overlapping controls and a page that scrolls sideways. The mock's texts are short words,
so nothing saw text leave its own box.

## Decision

- **`break-word` as the base.** `body` sets `overflow-wrap: break-word`, which every element
  inherits. A word longer than its box breaks inside it. Min-content widths are unchanged, so no
  box gets narrower than its words, and a layout where text fits is unchanged.
- **`anywhere` only where free text is a flex item.** The board card's status line
  (`.dl-card-line`) and a task's state on its overview and Git review (`.gt-state`) opt into
  `overflow-wrap: anywhere`, so their text item can shrink below its URL. Their icon sits beside
  the first line.
- **A sweep for the class.** `e2e/tests/text-fits.e2e.ts` gives the mock's tasks a long URL in a
  Needs you request and a long word in a title. It opens the responsive suite's pages, the task
  view and a task's Git review at 1440, 768 and 390 px, after loading and two animation frames, and fails on any of:
  - visible text past the nearest ancestor with a visible border or a background;
  - such a box past the next box around it;
  - the page or the window scrolling sideways;
  - a word of at most 12 characters broken across lines.

  A box that clips or scrolls holds its contents on purpose and ends the search. The last check
  guards the opposite failure: wrapping that squeezes labels.
- **What the sweep found besides the card is fixed too.**
  - The New task button's shortcut hint hides when the sidebar's top is under 240 px
    (a container query, since the sidebar can be dragged to 200 px at any window width).
  - Done's Archive all keeps only its icon when the task list is under 270 px, and keeps its
    name for screen readers.
  - The Tasks list's column names stay on one line.

## Alternatives

- **`overflow-wrap: anywhere` on `body`.** This was the first version of this change. The sweep's
  word check and the independent review showed that it lowers every min-content width. Short
  labels then broke mid-word wherever a row was tight: the 200 px sidebar's group names ("Done"
  over four lines, spilling out of its head), the pipeline filters, and table cells that used to
  scroll. Rejected.
- **Another per-class rule.** It would fix the one card and leave the class open. Rejected,
  although the two flex-row rules are per-class: the sweep is what keeps the class closed.
- **`word-break: break-all`.** It breaks every word at the end of a line, even a word that fits
  on the next line. Rejected.

## Consequences

- A long word in a block of text breaks inside its box. A long word in a flex or grid item still
  widens the item unless that item opts into `anywhere`. The sweep finds such a case on the swept
  pages when the mock shows long text there; the long text reaches the Git page, Tasks, the task
  view, the task overview and a task's Git review.
- Text that previously ran a little past a box that was too narrow, harmlessly, now breaks inside
  it. The Tasks column names hit this and were made `nowrap`. Other cases outside the swept pages
  may need the same.
- The existing per-class `anywhere` rules stay, each on purpose for its own element.
- The sweep adds about a minute of e2e time: 19 pages at 3 widths, about 2 s each.
