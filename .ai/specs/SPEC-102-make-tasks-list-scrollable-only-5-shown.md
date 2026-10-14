---
yulius-task: 01fea79c-7fd0-4cfa-96e5-6fdb8f3bf3f3
status: done
---

# SPEC-102: the "Run after" tasks show five at a time and say there is more

Status: done
Date: 2026-10-14
Owner: task #87
Scope authorization: the person's request (task #87): "Make tasks list scrollable, only 5 shown
by default, but it should be clearly visible that there's something to scroll." Screenshot: the
composer's start menu, its "After a task is done" group listing thirteen tasks (#85 … #73), so the
menu runs almost the whole height of the window.

## Goal

The start menu stays short however many tasks a project has: its task list shows five tasks and
scrolls the rest, and a person sees at a glance that more tasks wait below without having to try
scrolling.

## Context

- Every menu is built by `openMenu` (`crates/yulius-server/web/assets/dom.js`) and styled by the
  `.menu*` rules of `styles.css`. Items are flat: `header`, `separator` and choices.
- The composer's start menu (`openAfterMenu`, `taskview.js`) lists every live task of the sandbox
  under the header "After a task is done", one line each with its status (SPEC-038 MNU-006, no cut
  at 20 since the filter finds one). The automation editor's "After a task…" menu
  (`pages/automationedit.js`) listed the same candidates under the same header; since SPEC-093
  both pages open one shared start menu, `openStartMenu` (`startchoice.js`), which this change
  makes list its tasks as a group.
- SPEC-038 MNU-001 keeps a menu inside the window: the whole list (`.menu-list`) scrolls once the
  window is too short. With thirteen tasks the menu is ~1150 px tall, so on most windows the whole
  menu scrolls, and "When a rule is true…" sits below every task.
- A filter box appears from 8 choices (MNU-003); typing hides headers and separators and shows the
  matching items (MNU-004); arrows, Home/End and letters move focus (MNU-005), and `focus()`
  scrolls the focused item into view.
- The studio mock has nine live tasks; `&tasks=many` adds 30 more (39 rows).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| MSG-001 | A menu item `{ header, items }` is a group: its header, then its items in a box of their own (`role="group"`, named by the header) that shows at most 5 rows (`MENU_GROUP_ROWS`) and scrolls the rest. A group of 5 items or fewer shows all of them, with no scrolling and no cue. | `menus.e2e.ts` "the start menu shows five tasks …", "a group of five tasks or fewer …" |
| MSG-002 | While a group has rows out of sight it shows it without being scrolled: rows fade out at the edge they continue past (bottom, top or both), and a line under the box says how many rows are below ("↓ 8 more"); the line keeps its place but is blank once nothing is below (it hides only while a filter leaves no rows to scroll), so the menu does not jump. Clicking the line scrolls the box down by a page. The line is decoration for pointer users (`aria-hidden`): menu semantics already give the item count. | `menus.e2e.ts` "the start menu shows five tasks …", "scrolling to the end of the tasks …" |
| MSG-003 | The checked item of a group is in view when the menu opens, even past the fifth row; a menu that would focus a row of such a group first (no filter box, or a touch screen) focuses the checked row, so the focused row is never out of sight. | `menus.e2e.ts` "the picked task is in view …", "a group menu without a filter focuses its checked row …" |
| MSG-004 | Keyboard focus moved onto a group row (arrows, Home/End, a letter) scrolls the box so the row is wholly inside it, clear of the fade; the filter still shows only matching rows in the box (headers hidden), and the "more" line counts only the matching rows below. | `menus.e2e.ts` "arrows scroll the task box …", "the filter narrows the task box …" |
| MSG-005 | The composer's start menu and the automation editor's start menu list their tasks as such a group; every live task stays offered (MNU-006), the rows below the box ("When a rule is true…") stay visible without scrolling the whole menu on a window that fits the shortened menu, and an open menu still has no WCAG A/AA violations (MNU-007). | `menus.e2e.ts` "the start menu shows five tasks …", "the automation editor's … menu shows its tasks five at a time too", "in a short window the whole menu still scrolls …" |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The start menu shows five tasks in a scrolling box with a fade and a "more" line
- [x] The picked task, the keyboard and the filter work inside the task box, in both task menus
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- With the studio mock's nine tasks, the composer's start menu shows exactly five task rows inside
  the task box, the box scrolls, its bottom edge fades and the line under it reads "↓ 4 more".
- Scrolled to its end, the box fades at the top, not the bottom, and the line is blank; with
  `&tasks=many` the line reads "↓ 34 more" at the top.
- A task picked from beyond the fifth row is in view when the menu opens again.
- Pressing ↓ from the filter to the sixth task row scrolls the box so that row is wholly inside it.
- Typing a filter that matches two tasks shows both, without a fade and without the "more" line;
  a filter matching more than five scrolls to its end with the line blank and in place.
- A sandbox with five live tasks or fewer shows them all with no scrolling and no "more" line.
- "When a rule is true…" is visible without scrolling the whole menu at 1280 × 800.
- The menu tests from SPEC-038 and SPEC-094 still pass; `scripts/dev ci` and the e2e suite pass.

## Test plan

- e2e (`menus.e2e.ts`): the requests' cases above, measured in the page (rows inside the box's
  rectangle, `scrollHeight > clientHeight`, the fade class, the "more" line's text); the axe check
  on the open start menu; the existing placement, filter and scroll tests unchanged.
- e2e (`menus.e2e.ts`): the automation editor's start menu has the same group; a menu
  without a filter focuses its checked row (unit, in the page); a window too short even for the
  shortened menu still scrolls the whole list, keeps its place on resize, and the wheel passes
  from the box's end to the list.
- The whole e2e suite and `scripts/dev ci` for regressions.

## Decisions

- **A group is a header with items, scrolled on its own.** ADR
  [0083](../../docs/decisions/0083-long-menu-groups-scroll-five-rows.md). The task list is the one
  part of the menu that grows; capping only it keeps Start now, Supertask and "When a rule is
  true…" in view. Capping the whole menu at a height instead would hide those choices behind the
  tasks again. A generic `openMenu` group, not a task-specific widget, so the automation editor's
  menu and any later long list get the same behavior.
- **Five rows, measured.** The box's height is the height of its first five rows as drawn, so a
  row of any height (one with a description) still shows five whole rows. Five is the request's.
- **Two cues, both visible without interaction.** An edge fade alone is subtle on a dark menu, and
  macOS hides scrollbars until one scrolls, so the "↓ N more" line spells it out; the fade shows on
  which edge the list continues. The line stays in place (blank) at the end, so the menu does not
  jump while scrolling. The scrollbar is also kept thin and visible where the platform allows.
- **The filter searches every row, not the five shown.** It keeps MNU-004; the box keeps its cap
  while filtering, so a filter matching many tasks still scrolls with the same cues.
- **No "Show all" item.** It would add a choice that is not a choice and break the filter's "Enter
  picks the first shown item"; scrolling and the filter already reach every task.

## Open questions

None.

## Evidence

- Test first: the three SPEC-102 tests of the first slice failed on the old code (no group in the
  menu), the picked-task test failed before MSG-003 ("expected [#14 … #10] to contain #7 Search
  indexing"), and the review's regression tests failed before their fixes (focus on T1 out of view,
  the filtered "more" line hidden at the end, the wheel stopping at the box). All pass after.
- `cd e2e && E2E_TELEMETRY_DISABLED=1 npx e2e run tests/menus.e2e.ts`: 29 passed (the SPEC-038 and
  SPEC-094 tests among them; the #288 test now checks the task box, and a new one the whole list).
- `cd e2e && E2E_TELEMETRY_DISABLED=1 npx e2e run`: 296 passed, 1 skipped (`agent.e2e.ts`, needs a
  model key), 43 files.
- `scripts/dev ci`: exit 0; repository contracts passed (UI design tokens: the fade's mask stops are
  `--mask-solid` and `--mask-faint` in `tokens.css`); nextest 1520 passed, 16 skipped.
- Screenshots (mock, 1280 × 800): the start menu with nine tasks shows #14 … #10, the fifth row
  faded, "↓ 4 more" under the box and "When a rule is true…" in view; with `tasks=many`, scrolled,
  both edges fade and the line reads "32 more".
- Independent review (fresh session, `om-code-review`): round 1 requested changes (two majors: a
  focused row out of view, a weakened #288 test; minors: the filtered line jumping, overscroll
  containment, spec evidence); all fixed test-first; round 2 approved `fb193a7e`.
- Not tested here: macOS overlay scrollbars and real touch devices (the fade and the "more" line
  are the cues that do not depend on them).

