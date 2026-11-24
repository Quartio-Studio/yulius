# 0473. A menu opened over a modal dialog is mounted in it

## Status

Accepted for SPEC-644 (task #628, #1735).

## Context

`openMenu` (`dom.js`) appended every menu to `<body>`. Modal dialogs are shown with
`showModal()`: the dialog goes to the browser's top layer and everything outside it becomes inert.
A menu in `<body>` was therefore drawn under the dialog whatever its `z-index`, took no clicks, and
its first item could not take the focus. The sign-in terminal ("Sign in · <name>") cancels the
browser's own context menu, so a person who right-clicked to paste Claude's code had no usable
menu (#1735). Toasts had the same problem and were solved by `toastHost`, which mounts the toast
region in the topmost open modal dialog.

## Decision

While a modal dialog is open, `openMenu` mounts the menu in the topmost open modal dialog
(`topModal()`, shared with `toastHost`), and in `<body>` otherwise. The menu stays
`position: fixed` against the window, because `.modal` has no transform, filter or containment, so
`place()` puts it where it would be outside a dialog. A dialog that closes while its menu is open
closes the menu first, so no key or pointer handler of a removed menu stays on the document.
Escape keeps closing only the menu: its capture-phase handler prevents the key, so the dialog gets
no `cancel`.

## Alternatives

- Show the menu in the top layer with `popover="manual"` and `showPopover()`: it would be drawn
  above the dialog, but a modal dialog makes everything outside its subtree inert, top layer or
  not, so the menu would still take no clicks or focus.
- Mount the menu next to its anchor: a point anchor (the terminal's right-click) has no element,
  and an anchor inside a scrolling or clipped container would clip the menu.
- Open menus in modals as native `<dialog>`s of their own: a second modal changes focus trapping,
  backdrop clicks and Escape for every menu, far beyond this defect.

## Consequences

Every `openMenu` works while a modal dialog is open: the sign-in terminal's Copy, Paste and Select
all, and any menu anchored in a dialog. Menus without a modal dialog are unchanged. A future
`.modal` style that adds a transform, filter or `contain` would make the dialog the menu's
containing block and misplace it; the e2e test of the sign-in terminal checks the menu opens at the
pointer.
