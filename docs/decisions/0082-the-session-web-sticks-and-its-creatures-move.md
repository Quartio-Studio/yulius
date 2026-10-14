# 0082. The session's web sticks in its corner, its spider climbs in CSS, and a cockroach runs by from a timer

## Status

Accepted. Specified in [SPEC-101](../../.ai/specs/SPEC-101-spider-should-be-animated-and-the-web-sh.md).
It extends [0059](0059-a-session-is-printed-as-the-tui-in-html.md)'s spider-web corner of the
read-only session view (SPEC-001 UI-049).

## Context

A session's well has a pixel-art spider web floated in its top-right corner (SPEC-075). The person
asked for the spider to move, for the web to stick to the top while the transcript scrolls, and for
a pixel-art cockroach to run through the view now and then. The page scrolls in `.page`, the well
is `position: relative`, its entries are positioned and use `content-visibility`, and every loop in
the UI stops under `prefers-reduced-motion`.

## Decision

- The web stays a float and becomes `position: sticky; top: 0`. The float still makes the first
  lines wrap beside it. When the page scrolls, the sticky offset keeps it in the corner of the part
  of the well that is visible. Entries come after it in the document and are positioned, so further
  down the text paints over its dim threads.
- The spider moves with CSS keyframes in `steps()` on SVG groups: it climbs 6 pixels on an 8 s loop
  with two leg frames, and its thread is a rect of its own that `scaleY` shortens from the top, one
  whole pixel per step. A reduced-motion rule sets `animation: none` on all of them.
- The cockroach is a script (`scheduleCockroaches`, `runCockroach` in `pages/sessions.js`) on a
  random timer: 20–60 s after the session opens, then 45–150 s apart. It crosses the visible part
  of the well in 2–4 s at a random height and in either direction, with the Web Animations API, and
  is removed when it is out. It does not run under reduced motion or in a hidden page. The view's
  `destroy()` stops the timer and removes a running cockroach.
- The well clips horizontally with `overflow-x: clip`, so the cockroach never adds a scroll bar.
  `clip`, unlike `hidden`, creates no scroll container, so the web's sticky position still follows
  the page.

## Alternatives

- A second, absolutely positioned web moved by a scroll listener. That needs a listener and a copy
  of the web, and it lags on scroll. Rejected for `sticky`.
- SMIL `<animate>` for the spider. Its timing is exact, but the reduced-motion rule cannot stop it
  without a script. Rejected.
- A CSS-only cockroach on a long keyframe loop. Its path depends on the well's visible height and
  width, and a fixed loop is not "sometimes". Rejected for a script on a random timer.

## Consequences

- Text lower in the well can run over the stuck web. The threads are in the faint `--session-web`
  and the text paints above them, so reading is not hindered.
- One timer per open session view, stopped when the view is left. The cockroach is `aria-hidden`
  and never catches the pointer, so screen readers, selection and the keyboard are unaffected.
- `--session-roach` joins the session palette in both themes. It is decoration, so it has no
  contrast pair.
