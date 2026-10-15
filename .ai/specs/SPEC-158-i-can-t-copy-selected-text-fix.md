---
yulius-task: d2d08698-665d-44d4-b8c5-9189cdf8a403
status: done
---

# SPEC-158: text Claude Code selects in the web terminal reaches the clipboard

Status: done
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
| OSC-001 | An OSC 52 clipboard write (`Pc;<base64>`, any selection letters, UTF-8 text) from the program is written to the browser clipboard with a "Copied" notice, when this viewer can write and pressed a key or a mouse button in this terminal (or released one after such a press, anywhere) in the last 5 seconds. Answers xterm sends by itself do not count. |
| OSC-002 | When the browser refuses that write (or has no clipboard API), the notice says to press ⌘C (Ctrl+Shift+C elsewhere), and that key (or the right-click Copy) copies the program's text while xterm has no selection of its own; a newer xterm selection drops it. |
| OSC-003 | An OSC 52 query (`?`), an empty or malformed payload, a write to a read-only viewer and a write with no recent input from this viewer change nothing and are never answered: a program cannot read the clipboard, nor write it for a viewer who is only watching. |
| OSC-004 | xterm's own selection (Shift-drag, Option-drag on macOS) is copied as before, and keeps precedence over the program's copy for the copy keys. |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Full e2e suite green and the spec's evidence recorded

## Acceptance criteria

- In the mock task terminal (full screen, mouse reporting on, and a mock program that copies its
  drag selection with OSC 52 like Claude Code), a plain drag across "Read 142 lines" puts that text
  on the clipboard and shows "Copied".
- With the clipboard API refusing, the same drag shows the ⌘C / Ctrl+Shift+C hint and that key
  copies the text (the copy event carries it).
- An OSC 52 write the program prints without input from this viewer, and a `?` query, write
  nothing; a read-only viewer who just clicked into the terminal gets nothing either.
- After a refused copy, a Shift-drag (Option-drag) selection and the copy key copy that selection;
  once it is cleared, the copy key copies nothing (the program's copy went with it).
- The parser turns `c;<base64 of UTF-8>` into the text and `c;?`, `c;`, `c;%%%` and a payload
  without `;` into nothing.

## Test plan

- `e2e/tests/terminal.e2e.ts` (mock, Chromium): "a selection the program copies (OSC 52) reaches
  the clipboard" drives the drag, the refused write and ⌘C / Ctrl+Shift+C, and the unsolicited
  write; it fails before the fix (nothing written). "a read-only viewer's clipboard is never
  written by the program" fails when the `can_write` gate is removed. Unasked writes are followed
  by a marker line the test waits for, not a sleep. The clipboard key table test gains the OSC 52
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

## Evidence

- Root cause: Claude Code 2.1.292's `setClipboard` returns `ESC ]52;c;<base64>` (read from the
  installed CLI); the web UI had no OSC 52 handler (`grep` found none, and no xterm clipboard
  addon in `vendor/xterm`).
- Regression test first (f6c52b97): `npx e2e run tests/terminal.e2e.ts` failed: the key table
  test (`osc52Text is not a function`) and the OSC 52 test (`expected [] to equal
  ["StringContaining \"Read 142 lines\""]`, nothing written). After the fix (37493fa3): 14/14
  passed.
- Review round 1 (`om-code-review`, fresh session, head 37493fa3): REQUEST CHANGES, one major (no
  test for the read-only gate) and minors. Tests first (b56375c1): the stale program copy check
  failed (`expected 3 to be 2`); the read-only test failed with the `can_write` gate removed
  (`expected ["not yours"]`, a temporary mutation, reverted). Fixed in 2d58bdf3: 15/15 passed.
  Round 2 (head 2d58bdf3): APPROVE with nits; the cheap ones were taken in the next commit.
- `scripts/dev ci` (after `python3 scripts/bootstrap.py`), on 37493fa3: exit 0. Repository check,
  cargo check, fmt, clippy, nextest (1810 passed, 17 skipped), doctests, deny, shear and doc. The
  later commits touch JS, e2e tests and docs only; `python3 scripts/check_repository.py` passed on
  the final head after merging `origin/dev`.
- Full e2e suite (`cd e2e && E2E_TELEMETRY_DISABLED=1 npx e2e run`), on 2d58bdf3 plus the nits, at
  host load ~45: 460 passed, 1 skipped, 4 failed with timeouts (onboarding, rail,
  queued-messages and the terminal margin test, which failed at its first line before any
  clipboard code). `--last-failed`: 4/4 passed. Filed as #525 (same pattern as #506, #510).
  `terminal.e2e.ts` 15/15 again after merging `origin/dev`.
- `scripts/dev security`: exit 2, not run. Vidoc rejected the key (HTTP 401), as tracked by
  #470 (ADR-0104). Not passed.
- Not run here: Safari and Firefox (the e2e suite runs Chromium only; the refused write is
  simulated by a rejecting `writeText`), and real Claude Code in a browser (the mock reproduces
  its OSC 52 copy).
