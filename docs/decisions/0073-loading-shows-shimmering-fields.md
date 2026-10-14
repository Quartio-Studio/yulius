# 0073. Loading shows shimmering fields; a spinner marks only an action under way

## Status

Accepted. Specified in [SPEC-092](../../.ai/specs/SPEC-092-instead-of-loading-we-want-floating-load.md)
(task #77, SPEC-001 UI-050). It extends the calm loading of F 2026-10-10 (`skeleton` in `dom.js`)
to every place content loads.

## Context

The composer, the split card and the task view's steps drew shimmering bars while they loaded.
Everywhere else (the Git page, dialogs, Sessions, the editor pages, the task lists, the status
popover, Instance and more) showed a spinner ring and "Loading…" text. The web README already
named `skeleton` as the loading pattern, but `placeholder(text, 'loading')` and hand-written copies
of its markup drew the spinner. The request: "Instead of loading we want floating loading fields
in all places".

## Decision

- Content that is still loading shows the skeleton: shimmering bars where it will appear, its
  label (what loads) said to assistive technology only through `role="status"`.
  `placeholder(text, 'loading')` returns it, padded for a page, panel or dialog
  (`skeleton(label, lines, { block: true })`). There is no other loading markup. The label names
  what loads (`loader()` uses its `what`: "Loading the workflows…").
- Inline places use bars of the content's size: one bar for a home card's activity or an editor
  tree row, bars over a notepad's empty box or the browser panel's cover. Where the container's
  role allows only its own items (a `tree`), or many bars load at once (the home cards), the bars
  are decoration (`announce: false`) and the container is `aria-busy`.
- A spinner stays where nothing is drawn in place: an action under way (an update, the Jev key
  test), a terminal connecting, a working task's icon. A button busy with its own action ("Load
  more") says so on the button.

## Alternatives

- Change every call site to `skeleton(...)` and remove `placeholder(…, 'loading')`. More churn, and
  a later caller could bring the spinner back. Rejected for changing the one helper.
- A skeleton shaped like each page's content (cards, rows, columns). Closer to the final layout,
  but one shape per page drifts with the page and the loads take well under a second. Rejected
  for three bars, as the existing skeleton.
- Bars for actions too. An action has no content to stand in for, and a spinner says "working"
  where a shimmer says "arriving". Rejected.

## Consequences

- Every loading state looks the same and no "Loading…" text shows; screen readers still hear what
  loads.
- Tests look for `.skeleton[role="status"]` instead of `.placeholder-loading`, which is gone. The
  mock's `hold=<path>` and `hold=frames` keep a view loading for a test.
- Reduced motion stops the shimmer with every other animation.
