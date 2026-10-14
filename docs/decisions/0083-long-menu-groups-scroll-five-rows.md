# 0083. A long menu group shows five rows and says there is more

## Status

Accepted. Specified in
[SPEC-102](../../.ai/specs/SPEC-102-make-tasks-list-scrollable-only-5-shown.md). Builds on
[0030](0030-menus-filter-when-long.md) (menus stay in the window and filter when long).

## Context

The composer's start menu lists every live task of the project under "After a task is done"
(SPEC-038 MNU-006). With a dozen tasks the menu is about as tall as the window: the whole menu
scrolls, and "When a rule is true…" sits below every task. The person asked for the task list to
show five tasks and scroll the rest, with it clearly visible that there is something to scroll.

## Decision

- `openMenu` takes a group, `{ header, items }`: the header, then its items in a box of their own
  (`role="group"`, named by the header). A group of more than `MENU_GROUP_ROWS` (5) items is as
  tall as its first five rows as drawn and scrolls the rest; a shorter one shows all of them.
- While a group has rows out of sight, it says so without being scrolled: its rows fade at the
  edge they continue past, and a line under it reads "↓ N more" (the rows below), blank but in
  place at the end so the menu does not jump. A click on the line scrolls a page down. The
  scrollbar is thin and kept (`scrollbar-gutter: stable`) where the platform draws one.
- The checked row of a group is in view when the menu opens, and takes the focus when the menu
  would otherwise focus the group's first row (no filter box, or a touch screen); the keyboard and
  the filter work on the group's rows as on any other (the box keeps its cap while filtering).
- The start menu (`openStartMenu`, shared by the composer and the automation editor since SPEC-093)
  lists its tasks as such a group.

## Alternatives

- Cap the whole menu's height: the choices after the tasks (and before them, once scrolled) go out
  of sight again, which is the problem asked about.
- Show five tasks and a "Show all" item: a choice that is not a choice, which breaks the filter's
  "Enter picks the first shown item", while scrolling and the filter already reach every task.
- A fade or a scrollbar alone: a fade is subtle on a dark menu, and macOS shows a scrollbar only
  while scrolling; the counted line is the cue that cannot be missed.

## Consequences

- A menu has two nested scroll areas when the window is too short even for the shortened menu: the
  whole list (MNU-001) and the group's box.
- The "more" line is decoration (`aria-hidden`): a screen reader gets the item count from the
  menu's semantics, and the keyboard reaches every row.
- Any later long list in a menu (models, workflows) can become a group by giving its header items.
