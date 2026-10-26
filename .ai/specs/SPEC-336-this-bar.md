---
yulius-task: 507de216-9d57-4fc6-8cb9-ce4e9964d7da
status: in-progress
---

# SPEC-336: the sandbox tabs drag as smoothly as the Notes tabs

Status: in progress
Date: 2026-10-26
Owner: Yulius task #320, fixes #929
Scope authorization: user request "this bar should be as smooth as this bar. Fix." with two
screenshots: the sandbox tab row (All sandboxes, yulius, pomerado-rust, quartio-v2, product,
studio, +) and the Notes tabs (Team, Me, This sandbox), both showing the six-dot drag grip.

## Goal

Dragging a sandbox tab in the top row feels the same as dragging a Notes tab: the tab follows the
pointer, the tabs it passes slide aside, and on release it settles into its place.

## Context

- Symptom: the sandbox tab row reorders with native HTML5 drag and drop (`assets/layout.js`
  `createTabRow`: `draggable="true"`, `dragstart`/`dragover`/`drop` → `store.setSandboxOrder`).
  The browser draws a translucent ghost of the link, nothing in the row moves while dragging (an
  inset accent line marks the target), and on drop the row jumps to its new order.
- The Notes tabs (SPEC-112) and the Editor tabs (SPEC-127) share `assets/tabdrag.js`
  (`createTabDrag`): pointer events, transforms on the pressed tab and its neighbours, a FLIP
  settle on release, Escape to cancel, `clickConsumed()` so the release clicks nothing, and
  `dragging` so a redraw waits for the release.
- Root cause: SPEC-167 gave the sandbox tabs the Notes tabs' grip but kept their original native
  drag, so the two rows look alike and drag differently. The e2e test of SPEC-167 dispatched
  synthetic `DragEvent`s, which prove the reorder but say nothing about how it feels.
- `createTabDrag` finds its tabs with `[role="tab"]`; the sandbox tabs are links (`a.tab` with
  `aria-current`, in a `nav`), not a tablist, and "All sandboxes" is first and does not move.
- The row is redrawn on every `sandboxes` event (counts refresh often); a redraw during a press
  would replace the tab being dragged.

## Requirements

| ID | Requirement |
| --- | --- |
| SMO-001 | A sandbox tab pressed with a mouse or pen and moved past the drag threshold follows the pointer and the sandbox tabs it passes slide aside (`tabdrag.js`, the Notes tabs' drag); the browser's native link drag never starts. |
| SMO-002 | On release the row takes the new order (`store.setSandboxOrder`, remembered as before) and the tab settles into its place; the release opens no sandbox. "All sandboxes" never moves and nothing lands before it. |
| SMO-003 | Escape during a drag puts the tabs back and the release after it opens nothing; a plain click still opens the sandbox. |
| SMO-004 | A `sandboxes` update during a press does not redraw the row; it is drawn when the press ends. |
| SMO-005 | A drag that ends where it started writes no order, so sandboxes added later keep the server's order (as the native drop, which only wrote on a real drop). |

## Plan

- [x] Spec and issue for the janky sandbox tab drag
- [x] Failing e2e: sandbox tabs drag smoothly like the Notes tabs
- [x] Sandbox tab row uses the shared pointer drag
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-336-1: dragging a sandbox tab with the mouse moves it under the pointer and slides the others
  aside; releasing reorders the row without opening a sandbox.
- AC-336-2: Escape puts the dragged tab back; clicking a tab still opens its sandbox; the context
  menu still moves tabs.

## Test plan

- e2e `navigation.e2e.ts` "a sandbox tab follows the pointer, the others slide aside, and it
  settles where it is let go" (real mouse; replaces SPEC-167's synthetic `DragEvent` drop): the
  sandbox tabs are not `draggable`; a drag back to its place stores no order; mid-drag the pressed
  tab and the tabs it passes carry transforms; a server rename during the press reaches the store
  (the page title) but leaves the pressed tab in place, and is drawn on release (after a drag and
  after a cancelled plain press); release reorders without navigating and the transforms settle to
  none; Escape restores the order and opens nothing; the context menu moves a tab; a click still
  opens the sandbox; the order outlives a reload.
- The Notes and Editor drag tests keep passing (`tabdrag.js` gains an option only).

## Decisions

- Reuse `tabdrag.js` instead of tuning the native drag: the native ghost and the absence of live
  sliding cannot be styled away, and a second implementation would drift from the Notes one.
- `createTabDrag` takes an optional `selector` (default `[role="tab"]`) so the sandbox row passes
  `.tab[data-id]`: the links keep their link semantics, and "All sandboxes" (no `data-id`) is not
  part of the drag.
- No ADR: this applies the existing tab-drag decision (SPEC-112/SPEC-127) to one more row.

## Root cause and what else it could affect

Only the sandbox tab row used native drag; the workflow editor's step list also uses native drag,
but it is a vertical list with its own drop rules and is not in this request.
