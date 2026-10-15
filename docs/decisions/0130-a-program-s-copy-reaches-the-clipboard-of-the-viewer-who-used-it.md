# 0130. A program's copy (OSC 52) reaches the clipboard of the viewer who just used the terminal

## Status

Accepted. Specified in [SPEC-158](../../.ai/specs/SPEC-158-i-can-t-copy-selected-text-fix.md)
(task #149, fixes #519). Completes [0079](0079-the-terminal-mouse-is-the-programs.md) and UI-019.

## Context

Since ADR-0079 a writer's plain drag goes to a program that asked for the mouse. Claude Code's
full-screen view then draws the selection itself and copies it with an OSC 52 clipboard write
(`ESC ]52;c;<base64> BEL`). xterm.js handles OSC 52 only through its clipboard addon, which Yulius
did not load, so the person saw the text selected and could not copy it. One PTY is shown to
several viewers, and the program is an agent running code from a sandbox.

## Decision

- The web terminal handles OSC 52 writes itself (`termkeys.js` `osc52Text`, `terminal-core.js`):
  the decoded UTF-8 text goes to `navigator.clipboard.writeText` with a "Copied" notice.
- Only a viewer who can write and pressed a key or a mouse button in that terminal in the last
  5 seconds takes it. Other viewers, and writes the program prints on its own, change nothing.
- Queries (`?`) are never answered: no program reads a person's clipboard.
- A write the browser refuses (Safari without a user gesture, a plain-HTTP origin without the
  clipboard API) is kept, the notice says "Press ⌘C to copy" (Ctrl+Shift+C elsewhere), and that key
  copies it through the browser's copy command while xterm has no selection of its own; a newer
  xterm selection drops it.

## Alternatives

- xterm's clipboard addon: writes for every viewer at any time and answers reads unless
  configured otherwise; the fallback for refused writes would still be missing.
- Give the plain drag back to page selection: undoes ADR-0079, which the person asked for, and
  Claude Code's own copy (its `/copy`, Ctrl+C on its selection) would still be lost.
- Honour writes with no recent input: an agent could replace a watcher's clipboard (a pasted
  command) without them touching the terminal.

## Consequences

- Copying in Claude Code's full-screen view works as in a native terminal, in task terminals,
  sandbox shells and the Superagent panel; so do other programs that copy with OSC 52.
- A copy the program makes more than 5 seconds after the viewer's last key or mouse press or
  release is dropped (a release outside the terminal counts: a drag may end anywhere).
- Two writers who both used the terminal in those 5 seconds both get the copy. Telling whose drag
  it was would take the program's own state; the copy is text both could already see.
- The remaining risk is the native terminals' one: a program the person just started (`cat` of an
  untrusted file, `git log` in a hostile repository) can replace their clipboard in that window,
  so a later paste carries its text. Watchers, read-only viewers and idle writers are not exposed.
- A refused copy waits for the copy key until it is copied, a later write succeeds or xterm makes a
  selection of its own; the right-click Copy offers it too.
- Safari users may need one extra ⌘C after a drag; Chromium writes at once.
