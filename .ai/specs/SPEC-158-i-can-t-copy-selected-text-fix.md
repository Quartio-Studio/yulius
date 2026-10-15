---
yulius-task: d2d08698-665d-44d4-b8c5-9189cdf8a403
status: in-progress
---

# SPEC-158: text Claude Code selects in the web terminal reaches the clipboard

Status: in-progress
Date: 2026-10-15
Owner: task #149
Scope authorization: the person's request (task #149), "I can't copy selected text. Fix.", with a
screenshot of the Superagent panel: Claude Code's full-screen view shows a command highlighted as
selected, and nothing could be copied. Fixes #519.

## Goal

A selection made in a program that has the mouse (Claude Code's full-screen view) is copied to the
person's clipboard, as it is in a native terminal.

## Context

- **Symptom.** In a task terminal or the Superagent panel, a plain drag over Claude Code's output
  highlights the text, but the clipboard does not change, and ⌘C afterwards copies nothing.
- **Root cause.** Since SPEC-098 (ADR-0079) a writer's plain drag goes to the program when it asks
  for the mouse. Claude Code 2.1.292 then draws the selection itself and copies it the terminal way:
  an OSC 52 clipboard write, `ESC ]52;c;<base64> BEL` (its `setClipboard`; on a Linux sandbox
  without a clipboard tool it predicts `osc52`). xterm.js handles OSC 52 only through its clipboard
  addon, which Yulius does not load, so the sequence was dropped. xterm itself has no selection, so
  ⌘C (`copySelection`) found nothing to copy.
- **Why the tests missed it.** SPEC-098's tests checked that a plain drag reaches the program and
  selects nothing in xterm; the mock program never copied anything, unlike Claude Code.
- **Browsers.** `navigator.clipboard.writeText` outside a key or click handler works in Chromium
  with the page focused; Safari and Firefox may refuse it (no user activation), and an insecure
  (plain HTTP) origin has no `navigator.clipboard` at all. The browser's copy command
  (`document.execCommand('copy')` in a key handler) works everywhere.
- **What else the cause could break.** Every program that copies with OSC 52 (Claude Code's
  `/copy`, its Ctrl+C on a selection, Neovim and tmux with clipboard integration) had the same
  silent failure; the fix covers them all.

## Requirements

| ID | Requirement |
| --- | --- |
| OSC-001 | An OSC 52 clipboard write (`Pc;<base64>`, any selection letters, UTF-8 text) from the program is written to the browser clipboard with a "Copied" notice, when this viewer can write and sent the terminal input (a key or a mouse report) in the last 5 seconds. |
| OSC-002 | When the browser refuses that write (or has no clipboard API), the notice says to press ⌘C (Ctrl+Shift+C elsewhere), and that key copies the program's text while xterm has no selection of its own. |
| OSC-003 | An OSC 52 query (`?`), an empty or malformed payload, a write to a read-only viewer and a write with no recent input from this viewer change nothing and are never answered: a program cannot read the clipboard, nor write it for a viewer who is only watching. |
| OSC-004 | xterm's own selection (Shift-drag, Option-drag on macOS) is copied as before, and keeps precedence over the program's copy for the copy keys. |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- In the mock task terminal (full screen, mouse reporting on, and a mock program that copies its
  drag selection with OSC 52 like Claude Code), a plain drag across "Read 142 lines" puts that text
  on the clipboard and shows "Copied".
- With the clipboard API refusing, the same drag shows the ⌘C / Ctrl+Shift+C hint and that key
  copies the text (the copy event carries it).
- An OSC 52 write the program prints without input from this viewer, and a `?` query, write
  nothing.
- The parser turns `c;<base64 of UTF-8>` into the text and `c;?`, `c;`, `c;%%%` and a payload
  without `;` into nothing.

## Test plan

- `e2e/tests/terminal.e2e.ts` (mock, Chromium): "a selection the program copies (OSC 52) reaches
  the clipboard" drives the drag, the refused write and ⌘C / Ctrl+Shift+C, and the unsolicited
  write; it fails before the fix (nothing written). The clipboard key table test gains the OSC 52
  parser's cases.
- `mock.js`: the mock Claude Code copies a primary drag's text with OSC 52 on release, as the real
  one does.
- Gates: `scripts/dev ci` and the e2e suite.

## Decisions

- **Honour OSC 52 writes, only after this viewer's own input (ADR-0130).** Native terminals
  honour them; Yulius shows one PTY to several viewers, and an agent runs untrusted code, so a
  write is taken only by a viewer who can write and just used the terminal. Alternatives: xterm's
  clipboard addon (writes for every viewer at any time, and answers reads unless configured);
  turning the mouse back to selection (undoes SPEC-098, which the person asked for).
- **A refused write waits for the copy key** instead of being lost: the browser's copy command in a
  key handler needs no permission, so Safari and plain-HTTP origins work.
- **Never answer queries.** Reading the person's clipboard into a sandbox is a leak.

## Open questions

None.
