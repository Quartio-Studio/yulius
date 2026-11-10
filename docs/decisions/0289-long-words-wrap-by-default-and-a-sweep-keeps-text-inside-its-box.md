# 0289. Long words wrap by default, and a sweep keeps text inside its box

## Status

Accepted. Specified in [SPEC-449](../../.ai/specs/SPEC-449-fix-this-kind-of-issues-text-outside-the.md)
(task #431). Extends SPEC-001 UI-046 (layouts hold at every width). Follows SPEC-344, which fixed
one chip the same way.

## Context

A Git board card showed "Needs you: … at https://console.typesafe.ai/settings/billing, then tell
task #145", and the URL ran past the card's border. A word breaks only at spaces unless
`overflow-wrap` allows more, and in a flex or grid row the item holding the text is never narrower
than its longest word (`min-width: auto`). Nothing in the stylesheet allowed more for the page, so
each element showing a URL, a path or an identifier had to opt in. About forty rules had added
`overflow-wrap: anywhere` to one class each, every one after a report like this one, and the next
new element started without it. The layout checks (`responsive.e2e.ts`) look for overlapping
controls and a page that scrolls sideways, and the mock's texts are short words, so none of them
saw text leave its own box.

## Decision

- **One base rule.** `body` sets `overflow-wrap: anywhere`, inherited by every element. A word that
  does not fit its line breaks where it must, and the flex or grid item holding it can shrink
  below that word, so the row stays inside its box. Text meant to stay on one line is unchanged:
  `white-space: nowrap` (buttons, ellipsis rows), `pre`, the terminal and the editors do not wrap.
  The existing per-class rules stay; they are now redundant but harmless.
- **A sweep for the class.** `e2e/tests/text-fits.e2e.ts` gives the mock's tasks a long URL in a
  Needs you request and a long unbroken word in a title, opens every page of the responsive suite
  at 1440, 768 and 390 px, and fails when a visible text node runs past the nearest ancestor that
  draws a box (a border on both sides or a background), or when the page scrolls sideways. A box
  that clips or scrolls (an ellipsis, a wide table) holds its text on purpose and ends the search.
- **Fixed where the sweep found more.** The board card's status icon sits on its label's first
  line, and the New task button's shortcut hint gives way in the 200 px sidebar of a narrower
  window (the button's tooltip still names it).

## Alternatives

- **`overflow-wrap: break-word` as the base.** It breaks an overflowing word inside a block, but
  does not lower the min-content width, so a flex row such as the board card's line still grows
  past its box. Rejected: it misses the reported case.
- **Another per-class rule.** The fix for this card alone is one line, but it is the forty-first
  such line and leaves the class open. Rejected.
- **`word-break: break-all`.** It breaks every word at the line's end, even words that would fit
  on the next line. Rejected: it makes ordinary prose harder to read.

## Consequences

- When a box is too narrow for a word, the word breaks inside it rather than running out of it.
  A single-word label squeezed by its siblings in a flex row may now break mid-word instead of
  pushing past its row; the responsive suite's overlap checks and this sweep pass with it.
- A new element showing user or agent text needs no rule of its own, and the sweep catches one
  that still escapes (for example through `nowrap`) on the swept pages.
- The sweep adds about two minutes of e2e time (17 pages, 3 widths).
