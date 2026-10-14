---
yulius-task: 181be5d5-d438-443e-a01d-b78165270733
status: done
---

# SPEC-136: the Notes tabs show that they can be dragged

Status: implemented
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
| NOT-023 | Each Notes tab's title says it can be clicked or dragged: "Click to open, drag to reorder". The keys stay announced once, through `aria-keyshortcuts` (SPEC-112). | same e2e |
| NOT-024 | On a device with no fine pointer (no mouse or pen; touch cannot drag a tab) the grip is hidden, so no screen promises a drag it does not do; the context menu still moves tabs there. A touch-first device with a mouse or pen attached keeps it. | same e2e (the `not (any-pointer: fine)` rule) |

## Acceptance criteria

- AC-136-1: opening Notes shows a six-dot grip at the left of Team, This sandbox and Me; hovering a
  tab shows "Click to open, drag to reorder".
- AC-136-2: the tabs still read Team, This sandbox, Me to screen readers and tests; dragging,
  the keys and the menu still reorder them; the panel has no axe violations.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Notes tabs show a drag grip and a click-or-drag hint
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Design

`renderTabs` puts `h('span', { class: 'notes-tab-grip', 'aria-hidden': 'true' }, icon('grip',
{ size: 12 }))` before the label and a `title` on the tab. CSS: `.notes-tab-grip` is an inline
flex box in `--text-faint`, `cursor: grab`; `.term-tab:hover`, `.active` and `.dragging` lift it
to `currentColor`; `.notes-tabs.reordering .notes-tab-grip` is `cursor: grabbing`;
`@media not (any-pointer: fine)` hides it. The tab's left padding shrinks from 12 to 8 px, so each
tab grows by 12 px (the grip and its gap, less the padding).

## Test plan

- e2e `notes.e2e.ts` "the tabs show a grip and say they can be clicked or dragged" (fails before):
  every tab holds one visible `aria-hidden` grip `svg` before its name, `textContent` and the
  roles' names stay Team / This sandbox / Me, the title reads as NOT-023, the grip's cursor is
  `grab`; the grip is fainter than its tab at rest and takes the tab's colour when chosen, hovered
  and dragged (then with the `grabbing` cursor); a `not (any-pointer: fine)` rule hides it; axe
  finds nothing.
- The existing drag, keyboard, menu and Escape tests still pass (the grip is inside the tab, so a
  press on it drags the tab like any other press).
- Not covered: a real touch screen (the harness runs a fine pointer; the media rule is checked as
  CSS, as the forced-colours rules are).

## Decisions

- The grip goes on the tabs themselves, not a separate handle: the whole tab already drags, and a
  handle-only drag would take away what works today. The grip is a cue, not the only hit area.
- A title, not visible helper text: the strip has no room for a sentence, the workflow editor
  uses titles for the same hint, and the keyboard and menu routes stay announced through
  `aria-keyshortcuts` and the context menu.
- Hidden without a fine pointer (`not (any-pointer: fine)`, not `pointer: coarse`): touch does not
  drag these tabs (it scrolls the strip), so a grip there would promise something that does not
  happen, while a tablet with a mouse or pen attached can drag and keeps it.
- The `grab` cursor is on the grip only; the rest of the tab keeps the pointer cursor, since a
  click there is the commoner act and the tab strips elsewhere use it. The whole tab still drags.
- Review round 1 (fresh session, approve `a6d5bab2`, five minors, four nits): fixed test-first —
  the title no longer repeats the keys (screen readers heard them twice with `aria-keyshortcuts`),
  the grip is hidden by `not (any-pointer: fine)` instead of `pointer: coarse`, and the e2e now
  proves the hover, chosen and dragged colours and the `grabbing` cursor; the width note above made
  true. Kept: NOT-024 is checked as a stylesheet rule (the harness cannot emulate a device without
  a fine pointer), the tooltip may show after a drag lands (as in the workflow editor), and the
  plain cursor over the label.
- Confirmation review (fresh session, approve `9608231b`, nits only), kept: the dragged colour is
  also reached through `:hover` while the pointer rides the tab (the `grabbing` cursor is the part
  proven on its own); `not (any-pointer: fine)` is Media Queries 4 syntax, which older engines
  ignore, so they show the grip (safe); the padding reset beside the hidden grip is not asserted.
- No ADR: this applies an existing pattern (the workflow editor's grip) to one more strip; it is
  recorded here.

## Evidence

Run on the branch merged with `origin/dev` (2026-10-15):

- The new e2e "the tabs show a grip and say they can be clicked or dragged" failed before the
  change (no grip, no title) and again before the round-1 fix (the old title and media rule), then
  passed.
- `cd e2e && npx e2e run tests/notes.e2e.ts`: 35 of 35 passed (also rerun by both reviewers).
- `cd e2e && npx e2e run --workers 2`: 49 files passed, 1 skipped; 400 tests passed, 1 skipped.
- `scripts/dev ci` (fmt, clippy, nextest, doctests, cargo-deny, cargo-shear, rustdoc, repository
  check): exit 0, 1670 tests passed, 16 skipped; advisories, bans, licenses and sources ok.
- `scripts/dev security`: exit 2, not run. Vidoc answers HTTP 401 (Invalid or expired token)
  with the bound `VIDOC_API_KEY`; tracked in #470 (comment added). It is not a pass.
- Seen in the Yulius browser (mock, dark theme, 1280 × 800): the six-dot grip before Team, This
  sandbox and Me, faint on the tabs at rest and in the text colour on the chosen one.
- Not covered: a real touch-only device (the media rule is checked as CSS), browsers other than
  Chromium.
