# 0088. The session's web keeps its colours when stuck, and hangs in the well's corner

## Status

Accepted. Specified in [SPEC-109](../../.ai/specs/SPEC-109-web-is-not-aligned-to-the-corner-top-pad.md).
It supersedes the stuck colour of [0082](0082-the-session-web-sticks-and-its-creatures-move.md)
(`--session-web-stuck`, its observer and its mark); the rest of 0082 stands.

## Context

ADR-0082 made the session's pixel-art web sticky and, while stuck under the text, faded it to
`--session-web-stuck`, so that text crossing its threads stayed at AA. An IntersectionObserver
watched a zero-height mark at the well's top to know when it was stuck. The mark's `margin-top:
-16px` also lifted the float's start, so the web sat 16 px above the well's corner, over the
header. The person (task #94) found the web off the corner and "almost invisible … when scrolling,
it should stay in the same color".

## Decision

- The web keeps `--session-web` (threads, silk) and `--session-dim` (spider) whether it is stuck or
  not. `--session-web-stuck`, `.ss-web-stuck`, `watchStuckWeb` and `.ss-web-mark` are removed, with
  the contrast pairs `check_repository.py` held for the stuck colour.
- With the mark gone, the float starts at the well's padding and its negative margins put it exactly
  in the well's corner, inside the border; `position: sticky; top: 0` keeps it in the corner of the
  visible area.

## Alternatives

- Keep the fade and only move the mark out of the flow. That fixes the corner but not the colour the
  person asked for. Rejected.
- A right gutter beside the stuck web for the whole transcript, so text never crosses it. It takes
  84 px (56 px on phones) from every line. Rejected.

## Consequences

- Text lower in the well can run over the web's threads in their own colour. The text still paints
  above the web, and the threads are thin decoration, but where a glyph crosses a thread its
  contrast is lower than AA against that thread (about 2.9:1 for `--session-dim` in dark). This is
  the person's explicit choice.
- One observer fewer per open session view; the cockroach timer is the view's only one.
