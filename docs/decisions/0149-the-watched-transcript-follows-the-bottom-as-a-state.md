# 0149. The watched transcript follows the bottom as a state, kept through every change of height

## Status

Accepted. Specified in
[SPEC-185](../../.ai/specs/SPEC-185-this-screen-is-bugged-working-etc-is-bug.md) (#575); amends
[0145](0145-someone-else-s-task-is-watched-as-its-live-transcript.md): the octopus goes.

## Context

A watcher's live transcript (SPEC-179) scrolled to the bottom only when a read brought entries, and
decided whether the reader followed by measuring the distance to the bottom then (32 px). Heights
change between reads without a scroll: the `✻ Working…` line under the last line, the control card
above, side panels and the window, fonts, session blocks laid out late (`content-visibility`,
0143). The view was left above the bottom, the next read took the reader for one who had scrolled
up, and "New messages" replaced following for good (task #170). Chromium's scroll anchoring
sometimes moved the view along and hid the defect; Safari has none. The person who asked for the
octopus found it overkill, and its lane took 80 px of the transcript.

## Decision

- Following is a state: on from the first read; off only when the reader scrolls up away from the
  bottom; on again when they are back there (scrolling, or "New messages").
- One `ResizeObserver` on the scroller and its content puts a following view back at the bottom
  whenever either changes size, without a read.
- A read's new entries are followed while following; otherwise "New messages" shows, as before.
- No octopus and no lane over the watched transcript; it is inset 16 px, as the control card above
  it (12 px on phones).

## Alternatives

- A larger slack. Any fixed slack loses to a taller change (a three-line card, a large diff laid
  out late). Rejected.
- Scrolling down on every task event. Misses resizes, fonts and late layout, and pulls down a reader
  who scrolled up. Rejected.
- Relying on `overflow-anchor`. Safari has no scroll anchoring, and it anchors to the top line, not
  the bottom. Rejected.
- Keeping the octopus at rest only. Still the decoration the person asked to remove. Rejected.

## Consequences

- Only scrolling up stops following, so a scroll the browser makes for a content change (the end
  clamping it, anchoring) never does.
- A watcher with the page in the background reads nothing (0145); the observer only acts on
  layout, so it adds no work while nothing changes.
- The spider and cockroach of past sessions stay.
