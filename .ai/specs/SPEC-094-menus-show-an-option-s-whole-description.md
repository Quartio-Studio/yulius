---
yulius-task: 1af39197-2fe1-4e56-8907-6bd1ebba697b
status: in-progress
---

# SPEC-094: menus show an option's whole description

Status: in progress
Date: 2026-10-14
Owner: task #79
Scope authorization: the person's request (task #79): "In the web UI's drop-down menus, an
option's description is cut off with an ellipsis, so the person can't read it … Fix it for every
menu that shares this code (`openMenu` and its styles) … A menu item's description wraps onto as
many lines as it needs, and is never truncated. Labels can stay on one line. Keep the compact
menus from SPEC-038 (#287) … Check that the Supertask text (`SUPERTASK_HELP`) itself is complete
and reads well once it's fully visible." Screenshot: the composer's start menu, Supertask reading
"Waits for this project's running tasks, then ru…".

## Goal

Every option of every drop-down menu shows its whole description, so a person reads what the
option does before picking it. Menus stay as compact as SPEC-038 made them: inside the window, no
wider than today, and a short description still one line.

## Context

- Every menu is built by `openMenu` in `crates/yulius-server/web/assets/dom.js` and styled by the
  `.menu*` rules of `styles.css`. An item's `description` (or a disabled item's reason) is its
  second line, `.menu-desc`.
- SPEC-038 MNU-002 (ADR-0030) made that line `white-space: nowrap` with an ellipsis, the full text
  as the line's tooltip and wrapped only on the keyboard-focused item. Its reason was "Run after":
  every task was a two-line row and the menu ran past the window. Those rows have no description
  since SPEC-038 (MNU-006: label and status hint on one line), so descriptions remain only on the
  few choices that explain themselves (mode, workflow, start, model, effort, automation start).
- The menu is `max-width: 320px` and as wide as its widest content up to that (`position: fixed`,
  shrink-to-fit). Wrapping a line does not change its max-content width, so the menu keeps the
  width it has today; only lines longer than the room wrap.
- `SUPERTASK_HELP` (`taskview.js`) is both the Supertask item's description and the start chip's
  tooltip. It read "… then runs alone: its other tasks wait for it. For broad work like security
  hardening" — "its" reads as the supertask's and the last sentence had no end.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| MDS-001 | A menu item's description (or a disabled item's reason) wraps onto as many lines as it needs, focused or not, and is never cut or clipped; a word longer than the line breaks. Labels and hints stay on one line. This replaces SPEC-038 MNU-002. | `menus.e2e.ts` "a description wraps onto as many lines as it needs …", "the start menu shows Supertask's whole description, at desktop and phone width" |
| MDS-002 | Menus stay compact: a menu is no wider than 320 px (and than the window less 16 px), stays inside the window, and a description that fits on one line takes one line (its row no taller than 44 px). | `menus.e2e.ts`, the same tests and every SPEC-038 placement test |
| MDS-003 | A description shown whole carries no tooltip repeating it. | `menus.e2e.ts` "a description wraps …" |
| MDS-004 | The Supertask description reads as complete sentences: "Waits for this project’s running tasks, then runs alone while the project’s other tasks wait. For broad work such as security hardening." | `menus.e2e.ts` "the start menu shows Supertask's whole description …" |

## Plan

- [x] Spec, ADR and plan for whole menu descriptions
- [ ] Menu descriptions wrap and Supertask's text reads whole, at desktop and phone width
- [ ] Docs, review fixes and gates green

## Acceptance criteria

- An e2e test opens the composer's start menu at 1280 × 800 and at 390 × 844 and finds the
  Supertask description's whole text, with no ellipsis, nothing overflowing its line box and
  nothing outside the menu's list; the menu fits inside the viewport at both widths.
- A short description ("The agent plans the work itself") is one line, and the menu is no wider
  than 320 px.
- The SPEC-038 menu tests still pass; `scripts/dev ci` and the e2e suite pass.

## Test plan

- e2e (`menus.e2e.ts`): the start menu at desktop and phone width (the request's test); the
  workflow menu for a long description wrapping on an item that is not focused, a short one on one
  line, no tooltip on the description, the menu ≤ 320 px; the existing placement, filter and
  scroll tests unchanged.
- The whole e2e suite and `scripts/dev ci` for regressions.

## Decisions

- **Wrap, never truncate; keep the width.** ADR
  [0075](../../docs/decisions/0075-menu-descriptions-wrap.md). Wrapping changes no max-content
  width, so the 320 px cap and today's shrink-to-fit width hold, short descriptions stay one line,
  and only long ones take more lines. Widening the menu instead would still cut a longer text and
  make every menu wider; a hover or focus reveal (today's) hides the text from a pointer user who
  does not hover and from touch.
- **The window caps the width too.** `max-width: min(320px, 100vw - 16px)`: on a window narrower
  than 336 px a 320 px menu would cross an edge; placing it already keeps 8 px on each side.
- **Labels stay one line.** The request allows it, and a label is a name ("Supertask", "#12
  Checkout API"); "Run after" lists dozens of them, where one line each is what keeps it compact.
- **No tooltip on the description.** It repeated visible text; a screen reader would read it
  twice.
- **Supertask's text.** "its other tasks" read as the supertask's tasks; "the project's" says
  whose. Both sentences end with a period, as the automation editor's Supertask hint does.

## Open questions

None.
