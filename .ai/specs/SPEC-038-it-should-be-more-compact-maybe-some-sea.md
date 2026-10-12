---
yulius-task: 5f6c550e-a7bd-4adc-953e-8777cee33e4b
status: draft
---

# SPEC-038: menus are compact and filter themselves when long

Status: draft
Date: 2026-10-12
Owner: Yulius task #36
Scope authorization: user request: "it should be more compact, maybe some search or something,
same in all expandable widgets in the app" (screenshot: the composer's "Run after" menu, every
task a two-line row "After #35 I got something like this. … / Starts by itself when #35 is done ·
Queued", taller than the window).

## Goal

Every drop-down menu in the web UI takes as little room as its content needs and stays inside the
window. A long menu has a filter box at its top, so a person types a few letters instead of
scrolling. The "Run after" menu lists tasks as one-line rows.

## Context

- Every menu of the UI is built by `openMenu` in `crates/yulius-server/web/assets/dom.js`: the
  composer's Mode, Workflow, Run after, Model and Effort chips, the user menu, the task kebab and
  context menus, the editor's and the terminal's context menus and the Markdown slash menu. A
  change there reaches all of them; nothing else in the UI builds a drop-down list of items.
- Today a menu has no maximum height (a long one runs past the window's bottom edge) and an item's
  `description` wraps onto as many lines as it needs.
- "Run after" (`openAfterMenu` in `taskview.js`, TSK-012, SPEC-022) offers up to 20 live tasks of
  the sandbox, each "After #N title" with "Starts by itself when #N is done · <status>" under it.
- Native `<select>` controls (sandbox switcher on phones, folder pickers, the rule editor's task
  and state) are the browser's own lists with type-to-select; they are out of scope.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| MNU-001 | A menu is never taller than the window less 16 px: it opens below its anchor, or above it when there is more room there, and its items scroll inside it. | e2e |
| MNU-002 | An item's description is one line, cut with an ellipsis; the full text is the item's tooltip. Rows with a description are no taller than 44 px. | e2e |
| MNU-003 | A menu with 8 or more items to choose from has a filter box ("Filter", a search field) above its items, focused when the menu opens. Shorter menus have none and keep today's first-letter jumps. | e2e |
| MNU-004 | Typing in the filter shows only the items whose label, description or hint contain every typed word (case-insensitive); headers and separators hide while it has text; with no match the menu says "No matches". | e2e, in-page unit test |
| MNU-005 | From the filter, ↓ moves to the first shown item and Enter picks it; ↑ on the first item goes back to the filter; a letter typed on an item goes into the filter; Escape clears a filter with text, and closes the menu when it is empty. | e2e |
| MNU-006 | "Run after" lists each task on one line, "#N title" with its status on the right, under the heading "After a task is done", and offers every live task of the sandbox (no cut at 20) since the filter finds one. | e2e |
| MNU-007 | An open menu, filtered or not, has no WCAG 2.x A/AA violations. | e2e (axe) |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Every menu fits the window, with one-line descriptions and scrolling items
- [ ] A long menu has a filter box that narrows its items from the keyboard
- [ ] "Run after" lists tasks as one-line rows under one heading
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs, ADR and the changes log updated, delivered to dev

## Acceptance criteria

- In the studio mock, "Run after" opens with a focused filter; typing "check" leaves "#12 Checkout
  API" (and nothing unrelated); Enter picks it and the chip reads "After #12".
- At a 500 px tall window the menu's bottom edge stays inside the window and its items scroll.
- The Mode menu (4 items) has no filter and still jumps with first letters.
- No WCAG violations on an open filtered menu; `scripts/dev ci` and the e2e suite pass.

## Test plan

- In-page unit test of the pure `menuMatches(item, query)` (words, case, description and hint,
  empty query).
- e2e (`menus.e2e.ts`, new): filter on "Run after" (focus, narrowing, No matches, Escape, ↓/↑,
  Enter), letter typed on an item goes to the filter, short Mode menu without filter, menu height
  at a short window, one-line descriptions, axe on the open filtered menu.
- Existing suites (`intake.e2e.ts`, `new-task.e2e.ts`, `terminal.e2e.ts`) keep passing: they open
  and pick menu items by role and name.

## Decisions

- **One change in `openMenu`**, not per menu: the request says "same in all expandable widgets",
  and every menu goes through it.
- **Filter from 8 items** (MNU-003): below that the whole menu fits and first-letter jumps are
  enough; a box in a 4-item menu would add a row and steal its keyboard.
- **The filter sits outside the `role="menu"` list** (a `searchbox` labelled "Filter" with
  `aria-controls` on the list): ARIA menus may only own menu items, so the box is a sibling inside
  the same floating panel. Recorded in ADR-0027.
- **Descriptions are cut to one line** with the full text as tooltip rather than dropped: they
  still explain a choice at a glance (workflow steps, model notes).
- **Native selects stay native**: the browser's list already has type-to-select and phone pickers;
  replacing them is a separate, larger change with no reported pain.
