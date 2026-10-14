---
yulius-task: 181be5d5-d438-443e-a01d-b78165270733
status: in-progress
---

# SPEC-136: the Notes tabs show that they can be dragged

Status: in progress
Date: 2026-10-14
Owner: Yulius task #122
Scope authorization: user request: "here we want drag icon or click-or-drag if exists"
(screenshot: the Notes panel's tab strip, Team, This sandbox, Me).

## Goal

The Notes tabs can already be put in any order (SPEC-112 NOT-015), but nothing on them says so:
they look like ordinary tabs. Each tab now shows a drag grip, the same one the workflow editor's
steps use, and says "click to open, drag to reorder" when the pointer rests on it, so a person
finds the reordering without being told.

## Context

- `assets/notes.js` draws the strip (`renderTabs`): each tab is a `button role="tab"` whose text is
  the notepad's name. A press past 4 px drags it (pointer events, mouse and pen only); touch
  scrolls the strip and moves tabs from the context menu ("Move left" / "Move right"); Alt+←/→
  moves the focused tab (`aria-keyshortcuts`).
- The design system already has a drag cue: `icon('grip')` (six dots, `dom.js`), shown by the
  workflow editor's steps (`.wf-handle`, faint at rest, brighter on hover, title "Drag to
  reorder") and its library chips say "Add “X” (or drag it into place)" in their title. There is
  no "click-or-drag" component or cursor; the grip plus a title is the existing pattern.
- e2e tests read the tabs' names through `textContent` and `getByRole('tab', name)`: the grip must
  add no text and leave the accessible names as they are.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| NOT-022 | Each Notes tab shows the grip icon before its name: decorative (`aria-hidden`, no text, the tab's accessible name stays the notepad's name), faint at rest and in the tab's text colour when the tab is hovered, chosen or dragged, with the grab cursor over it (grabbing while dragged). | e2e `notes.e2e.ts` "the tabs show a grip and say they can be clicked or dragged" |
| NOT-023 | Each Notes tab's title says it can be clicked or dragged and names the keys: "Click to open, drag to reorder (Alt+← / Alt+→)". | same e2e |
| NOT-024 | Where the primary pointer is coarse (touch, which cannot drag a tab) the grip is hidden, so no screen promises a drag it does not do; the context menu still moves tabs there. | same e2e (the `pointer: coarse` rule) |

## Acceptance criteria

- AC-136-1: opening Notes shows a six-dot grip at the left of Team, This sandbox and Me; hovering a
  tab shows "Click to open, drag to reorder (Alt+← / Alt+→)".
- AC-136-2: the tabs still read Team, This sandbox, Me to screen readers and tests; dragging,
  the keys and the menu still reorder them; the panel has no axe violations.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] Notes tabs show a drag grip and a click-or-drag hint
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Design

`renderTabs` puts `h('span', { class: 'notes-tab-grip', 'aria-hidden': 'true' }, icon('grip',
{ size: 12 }))` before the label and a `title` on the tab. CSS: `.notes-tab-grip` is an inline
flex box in `--text-faint`, `cursor: grab`; `.term-tab:hover`, `.active` and `.dragging` lift it
to `currentColor`; `.notes-tabs.reordering .notes-tab-grip` is `cursor: grabbing`;
`@media (pointer: coarse)` hides it. The tab's left padding shrinks a little so the strip stays
about as wide as before.

## Test plan

- e2e `notes.e2e.ts` "the tabs show a grip and say they can be clicked or dragged" (fails before):
  every tab holds one visible `aria-hidden` grip `svg` before its name, `textContent` and the
  roles' names stay Team / This sandbox / Me, the title reads as NOT-023, the grip's cursor is
  `grab`, a `pointer: coarse` rule hides it, and axe finds nothing.
- The existing drag, keyboard, menu and Escape tests still pass (the grip is inside the tab, so a
  press on it drags the tab like any other press).
- Not covered: a real touch screen (the harness runs a fine pointer; the coarse rule is checked as
  CSS, as the forced-colours rules are).

## Decisions

- The grip goes on the tabs themselves, not a separate handle: the whole tab already drags, and a
  handle-only drag would take away what works today. The grip is a cue, not the only hit area.
- A title, not visible helper text: the strip has no room for a sentence, the workflow editor
  uses titles for the same hint, and the keyboard and menu routes stay announced through
  `aria-keyshortcuts` and the context menu.
- Hidden on coarse pointers: touch does not drag these tabs (it scrolls the strip), so a grip there
  would promise something that does not happen.
- No ADR: this applies an existing pattern (the workflow editor's grip) to one more strip; it is
  recorded here.
