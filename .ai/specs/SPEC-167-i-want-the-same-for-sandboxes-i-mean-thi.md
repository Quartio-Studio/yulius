---
yulius-task: 45f9e5a0-5e52-4ebc-8e8e-6e3a699aa597
status: in-progress
---

# SPEC-167: the sandbox tabs show that they can be dragged

Status: draft
Date: 2026-10-15
Owner: Yulius task #157
Scope authorization: user request: "i want the same for sandboxes - i mean this icon "you can
move it"" (screenshots: the Notes tabs with their six-dot grip, SPEC-136; the sandbox tab row,
All sandboxes, yulius, quartio-v2, product, studio, +).

## Goal

The sandbox tabs in the top row can already be put in any order by dragging one onto another (or
"Move left" / "Move right" in their context menu), but nothing on them says so. Each sandbox tab
now shows the same six-dot grip as the Notes tabs and says "Click to open, drag to reorder" under
the pointer, so a person finds the reordering without being told.

## Context

- `assets/layout.js` `createTabRow` draws the row: an "All sandboxes" tab (an icon, fixed first),
  then one `a.tab` per sandbox, `draggable="true"`, using native HTML5 drag and drop
  (`dragstart`/`dragover`/`drop` → `store.setSandboxOrder`). Its text is `.tab-name` plus an
  `sr-only` count and the visible running count and "needs you" dot. Its `title` says the
  repository and the access ("owner/name · write access"). The context menu moves a tab one place
  (WCAG 2.5.7). On a phone the row is a `<select>` instead (`.tabrow-tabs { display: none }`).
- SPEC-136 gave the Notes tabs the grip: `icon('grip', { size: 12 })` in an `aria-hidden` span
  before the name, `--text-faint` at rest, the tab's colour when hovered or chosen, `cursor: grab`,
  hidden under `@media not (any-pointer: fine)`; the title "Click to open, drag to reorder"
  (`TAB_HINT` in `notes.js`).
- e2e tests find a sandbox tab through `.tab-name` and the link's accessible name
  (`getByRole('link', /^product\b/)`): the grip must add no text.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| TAB-001 | Each sandbox tab (not "All sandboxes", which does not move) shows the grip icon before its name: decorative (`aria-hidden`, no text; the tab's accessible name and `.tab-name` stay as they are), faint at rest and in the tab's text colour when the tab is hovered or current, with the grab cursor over it. | e2e `navigation.e2e.ts` "the sandbox tabs show a grip and say they can be clicked or dragged" |
| TAB-002 | Each sandbox tab's title keeps the repository and access and adds, on its own line, "Click to open, drag to reorder", the same words as the Notes tabs (one shared constant). | same e2e |
| TAB-003 | On a device with no fine pointer (no mouse or pen) the grip is hidden, as on the Notes tabs (SPEC-136 NOT-024); the context menu still moves tabs there. | same e2e (the `not (any-pointer: fine)` rule) |

## Acceptance criteria

- AC-167-1: every sandbox tab in the top row shows a six-dot grip at the left of its name; hovering
  it shows the repository, the access and "Click to open, drag to reorder".
- AC-167-2: the tabs still read their names to screen readers and tests; dragging a tab onto
  another and the context menu still reorder them; the row has no axe violations.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Sandbox tabs show a drag grip and a click-or-drag hint
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Design

- `dom.js` exports `DRAG_TAB_HINT = 'Click to open, drag to reorder'`; `notes.js` uses it instead
  of its own `TAB_HINT`, so both strips say the same words.
- `layout.js` puts `h('span', { class: 'tab-grip', 'aria-hidden': 'true' }, icon('grip', { size:
  12 }))` before `.tab-name` in each sandbox tab and sets its title to
  `` `${repo} · ${access} access\n${DRAG_TAB_HINT}` ``.
- CSS: `.tab-grip` is an inline flex box in `--text-faint`, `cursor: grab`, `margin-left: -4px`
  (the tab keeps its 12 px padding, so the grip sits 8 px from the edge as on the Notes tabs);
  `.tab:hover .tab-grip, .tab.active .tab-grip` lift it to `currentColor`;
  `@media not (any-pointer: fine) { .tab-grip { display: none; } }`.

## Test plan

- e2e `navigation.e2e.ts` "the sandbox tabs show a grip and say they can be clicked or dragged"
  (fails before): every sandbox tab holds one visible `aria-hidden` grip `svg` as its first child,
  `.tab-name` text is unchanged, the link's accessible name is unchanged; the title ends with the
  hint on its own line after the repository and access; "All sandboxes" has no grip; the grip's
  cursor is `grab`; it is fainter than its tab at rest and takes the tab's colour on the current
  tab and when hovered; a `not (any-pointer: fine)` rule hides it; axe finds nothing.
- A drop of one sandbox tab onto another still reorders the row (the grip is inside the link, so
  a drag from it is the link's drag).
- The Notes grip test still passes with the shared constant.
- Not covered: a real touch screen (the harness runs a fine pointer; the media rule is checked as
  CSS, as SPEC-136 does).

## Decisions

- Same pattern as SPEC-136, not a new one: the person asked for "the same" icon. The grip is a cue
  inside the whole draggable tab, not a separate handle; the whole tab keeps dragging.
- The hint joins the existing title on a new line instead of replacing it: the repository and the
  access are the tab's only place for them, and the Notes tabs' wording is reused verbatim through
  one exported constant so the two strips cannot drift.
- No grip on "All sandboxes": it is fixed first and cannot be dragged.
- No `grabbing` cursor or `dragging` class while a sandbox tab moves: the sandbox row uses native
  drag and drop, during which the browser draws the drag image and owns the cursor.
- Hidden without a fine pointer, as on the Notes tabs, for one rule across both strips; phones show
  a `<select>` instead of the row anyway.
- No ADR: an existing pattern applied to one more strip, recorded here.
