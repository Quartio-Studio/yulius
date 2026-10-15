# 0143. A session view block is a keyboard region when it scrolls as laid out

## Status

Accepted. Specified in [SPEC-177](../../.ai/specs/SPEC-177-fix-350-380-363-the-session-view-says-wh.md);
task #137, #363. Amends the scroller rule of SPEC-075 (review round 5).

## Context

Long code, tool inputs, results, diffs and folded context in the read-only session view scroll
inside a 480 px box. A scrolling box must be reachable from the keyboard (axe
`scrollable-region-focusable`, WCAG 2.1.1): it gets `tabindex="0"`, `role="region"` and a name, as
the Git page's diffs do. SPEC-075 decided this while building the page, from an estimate of rows
(line length / 80, more than 16 rows). At 375 px or 400 % zoom lines wrap at about 38 columns, so a
block that scrolls got no tab stop; on a desktop a block of 17 short lines that fits was one. Entries
use `content-visibility: auto`, so blocks of entries far from the view are not laid out and cannot
be measured until they come near it.

## Decision

- Each scrolling-capable block carries its region name (`data-region`). Once laid out it is a region
  exactly when `scrollHeight > clientHeight`, checked by one `ResizeObserver` per session view when
  the block is first laid out and whenever its size changes (a rotation, a zoom, a panel beside it,
  a fold opened). The observer is disconnected when the view goes.
- Until a block is laid out (an off-screen entry, a closed fold) it keeps an estimate for the
  narrowest layout, 38 columns and more than 16 rows, so a block that may scroll is never skipped
  by the keyboard.
- Region names say whose block it is: `Result of Bash(npm test)`, `Input of …`, `Changed lines of
  path (n of m)` for several hunks, `Result (line N)` for a result whose call is on another page.

## Alternatives

- **Measure only.** A block skipped by `content-visibility: auto` would stay out of the tab order
  until the view nears it, so Tab could jump past a long code block between two calls.
- **Estimate for the narrowest layout only.** Simple and static, but on a desktop many blocks that
  fit would be tab stops that do nothing, against SPEC-075's "short blocks are no tab stops".
- **Give every block a tab stop.** Same noise, worse.

## Consequences

- Region attributes change after layout; tests wait for them (`toHaveAttribute`) instead of reading
  them right after the page builds.
- A block whose size never changes after its estimate keeps it only while it is not laid out.
- Code blocks inside messages keep the name `Code`.
