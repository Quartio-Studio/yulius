# 0097. Loading fields take the shape of what loads, drawn with the page's own layout

## Status

Accepted. Specified in [SPEC-120](../../.ai/specs/SPEC-120-floating-loaders-should-have-the-shape-a.md)
(task #106). It supersedes the "three bars, not per-page shapes" choice of
[ADR-0073](0073-loading-shows-shimmering-fields.md). The rest of ADR-0073 stands: shimmering fields,
the label said through `role="status"`, and spinners only for actions under way.

## Context

ADR-0073 made every loading state the skeleton: three bars in a box at most 640 px wide. It
rejected per-page shapes because one shape per page drifts from its page. The request, with a
screenshot of the Git page loading as three bars: "Floating loaders should have the shape and
layout of the final layout". Most pages render a few kinds of content: lists of item cards,
tables, grids of cards, editor forms and task lists. The Git page has a layout of its own.

## Decision

- A loading state is drawn in the shape of what replaces it, with the final layout's own classes
  (`.item-card`, `.table`, `.grid-2 > .card`, `.dl-card`, `.tl-columns` and others), so the page's
  CSS sizes the shape and the content lands where the fields were. A bar (`bar()` in `dom.js`) is
  one line of the text it stands for: it is 1lh tall, with the pill centred in it.
- `dom.js` holds the shared shapes: `items`, `item-section`, `table` (with its column count),
  `cards` and `form`. `placeholder(text, 'loading', shape)` and
  `loader(…, { shape })` use them. Text content (a prompt, a transcript, a log) keeps the `lines`
  shape: lines are its shape.
- A page with its own layout composes its shape from its own classes with `bar()` and
  `skeletonOf()`, next to the code that renders it: the Git page (status cards, Blocked, the
  board with the profile's columns, Activity), the sidebar task list and the Tasks page (its
  toolbar stays in place: tab names, chip outlines, a summary bar).
- Where a real class is a hook for tests or the keyboard (`.task-row`, `.tl-row`, `.task-group`,
  `.tl-link`), the shape uses a skeleton class that shares the real rule's selector in
  `styles.css`. Real-class selectors then find real content only.
- Parts of a page that do not depend on the read are drawn at once instead of after it: the
  Tasks column names, and the Subscriptions capacity panel, read beside the subscriptions instead
  of after them.

## Alternatives

- Keep three bars (ADR-0073). Rejected by the request: the page jumps when content lands, and the
  bars do not say what is coming.
- Skeleton CSS with copied sizes for each shape. It drifts as soon as a page's layout changes,
  which is the risk ADR-0073 named. Rejected for reusing the layout classes.
- Render the real components with empty data. Many have no empty-data form, and they would bring
  focusable controls and live regions into a loading state. Rejected.

## Consequences

- Loading content lands in place. `e2e/tests/loading-shapes.e2e.ts` compares the edges of a
  loading shape with the loaded content for item cards, a table, cards, the Git page and both
  task lists.
- A page whose layout changes changes its loading shape with it. A new layout needs its shape
  added (a SHAPES entry or a composed one), and `/design` shows a shaped sample.
- Shapes guess counts (three cards, four rows); the content can be longer or shorter than the
  guess, but it keeps its position and width.
