# 0030. Menus are compact and filter themselves when long

## Status

Accepted. Specified in
[SPEC-038](../../.ai/specs/SPEC-038-it-should-be-more-compact-maybe-some-sea.md). The one-line
description is replaced by [0075](0075-menu-descriptions-wrap.md): descriptions wrap.

## Context

Every drop-down menu of the web UI is built by `openMenu` in `web/assets/dom.js`: the
composer's chips (mode, workflow, run after, model, effort), the user menu, task menus and the
context menus of the terminal and editor. A menu had no maximum height and its descriptions
wrapped, so "Run after" with many live tasks (two lines each) ran past the window's bottom edge.
Finding one task meant scrolling the page. People asked for something more compact "with some
search", and the same in every such widget.

## Decision

- `openMenu` keeps every menu inside the window: it opens below its anchor, or above when there
  is more room there, and its items scroll inside it.
- An item's description is one line cut with an ellipsis; the full text is its tooltip
  (replaced by [0075](0075-menu-descriptions-wrap.md)).
- A menu with 8 or more items to choose from (`MENU_FILTER_MIN`) opens with a focused "Filter"
  search field (on a touch screen the first item takes focus, so the keyboard does not cover the
  menu). Typed words narrow the items by label, description and hint (`menuMatches`), not the
  tooltip. Headers and separators hide while it has text, and "No matches" is said in a
  status. ↓/↑ move between the filter and the items, Enter picks the first shown item, a letter
  or Backspace typed on an item edits the filter, keys composing text in an input method stay in
  the filter, and Escape clears the filter before it closes the menu. A window that only gets
  shorter (a phone keyboard) no longer closes a menu; a change of width still does.
- The filter is a sibling of the `role="menu"` list inside the floating panel, because an ARIA
  menu may own only menu items.
- "Run after" lists each live task on one line, "#N title" with its status, under the header
  "After a task is done". It no longer stops at 20 tasks.

## Alternatives

- A filter in every menu: a 3-item menu would gain a row and lose its first-letter jumps.
- A filter only in "Run after": the request asked for the same in every expandable widget, and
  other long menus (models, the slash menu) benefit too.
- A combobox (`role="combobox"` with a `listbox`) instead of a menu: the menus run actions and
  hold radio and checkbox items, which a listbox cannot express. Changing every caller's semantics
  is out of proportion.
- Replacing native `<select>` controls with the same widget: they already type-to-select and use
  the phone's picker, and nobody reported them.

## Consequences

- Every current and future menu gets the behaviour without code in its caller.
- With focus in the filter, Home and End move the text cursor instead of jumping to the first or
  last item; ↓ and ↑ reach the items.
- A filtered menu keeps the width it opened with, and a menu above its anchor stays attached to
  it while it shrinks.
