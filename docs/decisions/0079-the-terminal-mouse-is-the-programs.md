# 0079. The terminal's mouse is the program's when it asks for it, with no mouse chip

## Status

Accepted. Specified in [SPEC-098](../../.ai/specs/SPEC-098-this-is-weird-af-delete-it-default-shoul.md).
Changes UI-019 (F 2026-10-07: terminal text behaves like page text) and replaces SPEC-027's
forwarding of plain clicks and wheel turns.

## Context

Since F 2026-10-07 the web terminal kept the mouse for text selection: `mousefilter.js` removed a
program's mouse-reporting switches from the output. Claude Code's full-screen view needs the mouse
(the wheel scrolls its transcript, a click expands a tool group), so the terminal forwarded wheel
turns (#34, #78) and plain single clicks (SPEC-027) itself, and a hover chip ("Mouse: select text ·
send to app") handed every mouse event over. The person found the chip weird and asked for the
mouse to go to the program by default (task #83).

## Decision

- A viewer who can write gets the program's mouse reporting as the program asked: xterm.js reports
  clicks, drags, the wheel and right-clicks in the program's encoding, as xterm, iTerm2 and GNOME
  Terminal do. The chip and the manual forwarding are removed.
- Shift-drag (Option-drag on macOS) selects text while the program has the mouse (xterm's forced
  selection); the selection is copied at once as before. With the same key held, a right-click
  opens the terminal's Copy, Paste and Select all menu; a plain right-click is the program's.
- A read-only viewer cannot send anything, so the filter still removes the switches for it and its
  mouse selects text. The terminal switches when write access arrives or goes.
- A press in the margin beside another viewer's centred grid is kept from xterm (which would report
  it at the grid's edge); it only takes the size. Ctrl+wheel and horizontal swipes stay with the
  browser.

## Alternatives

- Delete the chip and keep the mouse for selection: the opposite of the default the person asked
  for, and Claude Code's drag selection, double-clicks and other mouse use would stay unreachable.
- Keep the chip with the program's mouse as its default: still the chip that was called weird, for
  what one held key already does in every native terminal.
- Give read-only viewers the program's mouse too: their mouse would then do nothing at all.

## Consequences

- Claude Code behaves as in a native terminal; plain drags and double-clicks go to it, so selecting
  page text in a full-screen program needs Shift (Option on macOS).
- Less code: no forwarding of wheel and click reports, no click-count tracking, no chip.
- Mouse reports still neither count as typing nor take control (ADR-0052); a settled click still
  takes the size. A mouse report lost while disconnected shows no "Not connected" notice; typing
  still does.
- A click on a link (a web link or a file path) also reaches the program, as a click there does in
  a native terminal: xterm reports every press and its link handling cannot hold one back. In
  Claude Code a click on a path inside a collapsed tool group opens the file and toggles the group.
