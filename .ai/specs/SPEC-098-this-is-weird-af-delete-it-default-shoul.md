---
yulius-task: c44bfbbc-18b4-4207-b597-fe54df6f5e9f
status: in-progress
---

# SPEC-098: the terminal's mouse goes to the program, with no mouse chip

Status: in progress
Date: 2026-10-14
Owner: task #83
Scope authorization: the person's request (task #83), with a screenshot of the terminal's
floating "Mouse: select text · send to app" chip: "this is weird af, delete it. default should be
sending things to the terminal".

## Goal

The web terminal handles the mouse like a native terminal: when the program asks for the mouse
(Claude Code's full-screen view does), clicks, drags and the wheel go to the program, and no chip
offers to switch. Text selection stays one modifier away (Shift-drag, Option-drag on macOS), as in
xterm, iTerm2 and GNOME Terminal.

## Context

- **Today (F 2026-10-07, SPEC-027, #34, #78).** `mousefilter.js` strips the program's
  mouse-reporting switches (DECSET 1000/1002/1003/1006 …) from the output so text behaves like
  page text. To let Claude Code still work, `terminal-core.js` forwards wheel turns and plain
  single clicks itself (`reportsToApp`, `wheelReport`, `clickReport`). A hover chip in the
  terminal's corner ("Mouse: select text · send to app") hands every mouse event to the program
  instead. The chip is what the person calls weird; the default they want is the program's.
- **xterm.js already does the native thing.** With mouse reporting on, it reports presses,
  releases, drags and wheel turns in the program's encoding, and `shouldForceSelection` lets a
  Shift-drag (Option-drag on macOS with `macOptionClickForcesSelection`, already set) select text.
  The selection is copied at once (unchanged: `onSelectionChange`).
- **Read-only viewers.** xterm turns selection off while mouse reporting is on, and a read-only
  viewer's input is dropped (`disableStdin`, and the server refuses it). Handing them the mouse
  would make it do nothing; they keep plain selection.
- **Who sets the size (ADR-0052).** Mouse reports are not typing on either side (`typing.js`,
  `is_typing`), so a click reported by xterm does not take control; the settled-click claim in
  `terminal-core.js` is unchanged.
- **Margins.** Another viewer's grid is centred in this box (SPEC-082). xterm's element fills the
  box and clamps a press in the margin to column 1 or the last column, so the program would get a
  click the person did not aim at it; today `cellAt().inside` prevents that.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| MSE-001 | The terminal shows no mouse chip or toggle (`.term-mouse-toggle` and its CSS are gone). | `terminal.e2e.ts` "a click on Claude Code's output expands it" |
| MSE-002 | A viewer who can write gets the program's mouse reporting as the program asked: xterm reports clicks, drags, the wheel and right-clicks in the program's encoding. A plain click on Claude Code's collapsed tool group expands it, a second collapses it; a plain drag reaches the program and selects nothing. | same |
| MSE-003 | Shift-drag (Option-drag on macOS) selects text while the program has the mouse, and the selection is copied as before; with Shift (Option on macOS) held, a right-click opens the terminal's Copy, Paste and Select all menu. Without mouse reporting nothing changes (plain drag selects, right-click opens the menu). | "Ctrl+C copies a selection …" (now with the modifier), new assertions in the click test |
| MSE-004 | A read-only viewer keeps the mouse for text selection: the reporting switches are still filtered for it, a drag selects, nothing is sent. When write access arrives or goes, the terminal switches mode at once. | "a read-only viewer's click does not reach the program" (plus a drag that selects) |
| MSE-005 | A press in the margin beside another viewer's centred grid reaches no program; it only takes the size. | "a click beside another viewer's grid …" (unchanged test) |
| MSE-006 | Ctrl+wheel (pinch-zoom) and horizontal swipes stay with the browser while the program has the mouse (#78): xterm neither reports nor cancels them; a plain wheel turn reaches the program as a wheel report. | "the wheel scrolls the program; pinch-zoom and horizontal swipes stay with the browser" |
| MSE-007 | The manual wheel and click forwarding (`reportsToApp`, `wheelReport`, `clickReport`) is removed: xterm reports them now. | the encoding unit test is removed with `clickReport`; MSE-002 covers the behaviour |
| MSE-008 | A plain right-click while the program has the mouse opens no menu (neither Yulius's nor the browser's); a mouse report lost while disconnected shows no notice, a typed key still does. | the clipboard test (`defaultPrevented`), "mouse use while disconnected says nothing" |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] A writer's mouse goes to the program by default, without the chip
- [ ] Read-only viewers and the margin beside another viewer's grid keep the mouse off the program
- [ ] Product spec, ADR-0079 and the docs say the mouse is the program's
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- In the mock task terminal (full screen, mouse reporting on), no `.term-mouse-toggle` exists; a
  click on "Searched for 2 patterns, read 1 file (click to expand)" expands the group, a second
  click collapses it; a plain drag across a row selects nothing.
- A Shift-drag (Option-drag on macOS) across a row selects it and Ctrl+C (⌘C) copies it; a
  modified right-click opens Copy, Paste and Select all.
- In a read-only terminal a click sends no input frame and a drag selects text.
- A click in the margin beside another viewer's 70×30 grid takes the size and does not toggle the
  group.

## Test plan

- `e2e/tests/terminal.e2e.ts` (mock, Chromium): the click test above rewritten for the new
  default (fails while the filter keeps the mouse: a plain drag selects text and the chip exists);
  the clipboard test selects with the modifier and opens the menu with it; the read-only test also
  drags and expects a selection; the margin test unchanged (fails without the margin guard once
  xterm reports the mouse).
- `crates/yulius/tests/live_ui_browser.rs` (opt-in Chrome) keeps checking the filter's default
  (read-only viewers): rows are HTML and a double-click selects a word.
- Gates: `cargo` fmt, clippy and tests, `scripts/check` as AGENTS.md lists, and the e2e suite.

## Decisions

- **The program gets the mouse when it asks for it, for writers (ADR-0079).** Native terminals do
  this, and Claude Code's full-screen view is built around it (clicks expand tool groups, its own
  drag selection, wheel scrolling). Alternatives: keep page-like selection and only delete the
  chip (the person asked for the opposite default); keep the chip but flip its default (still the
  chip they called weird).
- **Selection with Shift (Option on macOS), not a chip.** It is xterm's and the native terminals'
  convention, needs no UI and is already configured.
- **Read-only viewers keep selection.** They cannot send anything, so the program's mouse would
  make the mouse dead for them; the filter stays for exactly this case.
- **Right-click is the program's under mouse reporting; the modifier brings the menu.** Same rule
  as selection, so one modifier means "the page, not the program".
- **The margin guard is a capture-phase `mousedown` that stops xterm from seeing presses outside
  the grid** (and focuses the terminal itself), rather than patching xterm's coordinates.

## Open questions

None.
