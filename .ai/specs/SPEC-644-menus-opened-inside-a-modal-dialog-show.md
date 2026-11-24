---
yulius-task: 1be160e2-e214-43f5-8c56-57c9463c183f
status: in-progress
---

# SPEC-644: Menus opened inside a modal dialog show above it and work

Status: in-progress
Date: 2026-11-24
Owner: Yulius task #628
Scope authorization: task #628: "Solve GitHub issue #1735 in one pull request into `dev` whose
description says `Fixes #1735`." No new issues may be created; findings go to #1735.

## Goal

A right-click in the subscription sign-in terminal ("Sign in · <name>", a native modal `<dialog>`)
opens the terminal's Copy / Paste / Select all menu above the dialog, where a pointer can click it
and the keyboard can reach it (#1735). The same holds for every `openMenu` while a modal dialog is
open. Menus outside dialogs keep their placement and behaviour.

## Context

- **Symptom.** In the sign-in dialog, the terminal's right-click menu is drawn but cannot be
  clicked: Playwright reports `<dialog open class="modal modal-full" …> subtree intercepts pointer
  events`. Its first item does not take the focus, so the keyboard cannot reach it either. The
  terminal cancels the browser's own context menu, so the person has no menu Paste at all (⌘V and
  Ctrl+V still work).
- **Root cause.** `openMenu` (`crates/yulius-server/web/assets/dom.js`) always mounts the menu with
  `document.body.append(menu)`. `openModal` shows its dialog with `dialog.showModal()`: the dialog
  goes to the browser's top layer, and everything outside it is inert. A menu in `<body>` is below
  the top layer whatever its `z-index` (`.menu`, `--z-menu`), and inert, so `first.focus()` does
  nothing.
- **Why the tests missed it.** The menu specs (`menus.e2e.ts`, `editor.e2e.ts`, `terminal.e2e.ts`,
  `filedrop.e2e.ts`) open menus outside dialogs only. No test opened a menu from inside a modal.
- **The same problem, solved before.** Toasts had it too: `toastHost` in `dom.js` moves the toast
  region into the topmost open modal dialog and back to `<body>` when it closes.
- **Other menus in modals.** Any `openMenu` whose anchor sits in a modal had the same defect. The
  terminal's menu is the one people meet: the sign-in dialog is the only modal that holds a terminal.
- **Integration gate repair (assigned to this task by Yulius).** Dev `ee5ab98c4` failed its gate
  (Yulius CI run 93f09d24, `test (2/2)`) on
  `yulius-browser xclipboard::tests::native::the_paste_owner_answers_only_its_text_and_lets_it_go`
  ("the held paste was not let go", `native.rs:279`), a test from SPEC-639 unrelated to menus. The
  paste owner releases the CLIPBOARD on its X connection (`conn`) without waiting for a reply, and
  the test reads the owner on a second connection (`other`) after only `conn.flush()`. The X server
  serves its clients independently, so it may answer `other`'s read before handling `conn`'s
  release. Reproduced 1 in 159 `--stress-count` runs with the same message. The product is right:
  nothing in it reads the release back. The test now makes a round trip on `conn`
  (`GetInputFocus`) before the read; 2000 of 2000 stress runs pass.

## Requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| MDM-001 | A menu opened while a modal dialog is open is mounted in the topmost open modal dialog, so it is above the dialog, clickable and focusable; its place on screen is the same as outside a dialog. | `subscriptions.e2e.ts` "the sign-in terminal’s right-click menu shows above its dialog…", "a menu and a toast opened over two modal dialogs…" |
| MDM-002 | Its keyboard behaviour is the menu's: the first usable item takes the focus, arrows move it, Enter picks; Escape closes only the menu, not the dialog. | the same test |
| MDM-003 | A dialog that closes while its menu is open closes the menu too (no key handler is left behind). | the same test |
| MDM-005 | Escape and Tab on a menu opened at a point (no anchor element) give the keyboard back to the element that had it when the menu opened (review round 1; on dev it went to the page, also outside dialogs). | `subscriptions.e2e.ts` "the sign-in terminal’s right-click menu…" (Escape, Tab) |
| MDM-004 | Menus opened without a modal dialog are still mounted in `<body>` and placed as before. | `menus.e2e.ts`, `terminal.e2e.ts`, `editor.e2e.ts`, `filedrop.e2e.ts` unchanged and passing |

## Plan

- [x] Regression e2e test: the sign-in terminal's menu Paste by pointer and keyboard, failing on dev
- [x] Menus opened in a modal dialog mount in it and close with it
- [x] Menu specs outside dialogs pass; decision record, docs and changes entry
- [x] Independent review in a fresh session, every finding answered
- [ ] Integration gate repair: the xclipboard paste owner test reads the release after a round trip
- [ ] Review notes: a point menu gives the keyboard back, topmost of two modals tested, comments and ADR wording

## Acceptance criteria

- A right-click in the sign-in terminal opens a menu whose Paste can be clicked and reached with
  the keyboard.
- Menus opened outside dialogs keep their placement and behaviour.
- An e2e test covers the sign-in terminal's menu Paste.

## Test plan

- New e2e test in `e2e/tests/subscriptions.e2e.ts` (Chromium, mock): open the sign-in, stub the
  clipboard's text, dispatch `contextmenu` on `.login-terminal .xterm-screen`; the menu is at the
  pointer; clicking Paste types the code into the mock sign-in. Open it again: Paste is focused,
  ArrowDown/ArrowUp move, Enter pastes again. Escape closes only the menu (no cancelled sign-in). A
  sign-in that ends with the menu open removes the menu and leaves Escape to the page.
- Shown failing on dev's `dom.js` before the fix.
- Run `subscriptions.e2e.ts`, `menus.e2e.ts`, `terminal.e2e.ts`, `editor.e2e.ts` and
  `filedrop.e2e.ts` in full.

## Decisions

- **Mount the menu in the topmost open modal dialog** (as `toastHost` does), not in the top layer
  with `popover="manual"`. A popover outside the modal dialog is still inert while the dialog is
  modal (inertness follows the DOM, not the top layer), so it would show but not take clicks or
  focus. Inside the dialog the menu keeps `position: fixed` against the viewport (the dialog has no
  transform, filter or containment), so `place()` needs no change. Recorded as
  [ADR-0473](../../docs/decisions/0473-a-menu-opened-over-a-modal-dialog-is-mounted-in-it.md).
- **Escape.** The menu's capture-phase keydown handler already calls `preventDefault()` and
  `stopPropagation()` on Escape. A prevented Escape keydown starts no close request, so the
  dialog's `cancel` does not fire; the test holds it.
