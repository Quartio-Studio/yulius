---
yulius-task: 8501149c-5fee-4b63-b82a-a4f501db7a60
status: done
---

# SPEC-280: A wheel turn scrolls Claude Code as far as it scrolls a page

Status: done
Date: 2026-10-23
Owner: task #266
Scope authorization: task #266: "I can't scroll up in Claude conversation for some reason, fix".
Fixes #863. Changes MSE-006 of [SPEC-098](SPEC-098-this-is-weird-af-delete-it-default-shoul.md)
(ADR-0079: the terminal's mouse is the program's).

## Goal

In a task's terminal and in the Superagent, the mouse wheel and the trackpad scroll Claude Code's
conversation as far as the same gesture scrolls a web page, as in a native terminal.

## Context

Confirmed against `dev` at acb1adba.

- **Symptom.** Scrolling up in Claude Code's conversation barely moves it. Measured in the mock UI
  (row height 20 px), with the wheel reports the terminal sent and the rows Claude Code scrolls:

  | Gesture | Distance | Reports = rows scrolled |
  | --- | --- | --- |
  | 10 macOS mouse notches (Chrome: 40 px each) | 400 px = 20 rows | 5 |
  | trackpad swipe, 60 events of 10 px | 600 px = 30 rows | 10 |
  | 5 Windows or Linux notches (100 px each) | 500 px = 25 rows | 5 |
  | 5 Firefox notches (3 lines each) | 15 rows | 5 |

- **Root cause.** Claude Code 2.1.292 runs full screen (alternate screen) and turns on mouse
  reporting (1000, 1002, 1003, 1006), so a wheel turn reaches it as an SGR wheel report
  (`ESC [ < 64 ; col ; row M` up, `65` down). Run in a PTY, it moves its transcript **one row per
  report**. It moves 3 rows per report only for terminals it recognises as xterm.js, through
  `TERM_PROGRAM=vscode` or an XTVERSION answer starting `xterm.js`. Yulius sets neither, and
  xterm.js 6.0 does not answer XTVERSION. xterm.js 6.0 sends **at most one report per browser
  wheel event** (`consumeWheelEvent` only decides whether it is zero) and scales pixel deltas under
  50 px down to 0.3×. So one mouse notch scrolls one row of a conversation hundreds of rows long,
  and a trackpad swipe scrolls about a third of its distance. The client path itself works: the
  reports reach the program (`terminal.e2e.ts` "the wheel scrolls the program"), and the server
  forwards them unchanged (`terminal_input`, not typing).
- **Why the tests missed it.** The e2e test checks only that *a* wheel report is sent
  (`some(startsWith('\x1b[<64;'))`), not how many. The mock agent scrolled its transcript by three
  lines per report, so UI work on it never showed the distance; it now scrolls one, as Claude Code.
- **Same cause elsewhere.** Every full-screen program with mouse reporting (vim with `mouse=a`,
  `less --mouse`, htop) in a Superagent or Cockpit shell terminal scrolled one step per wheel
  event too. They share `terminal-core.js` and are fixed with it.

## Requirements

- **WHL-001** While the program has the mouse, a plain wheel turn (no Ctrl, Shift, Alt or Meta,
  a vertical delta) is reported as one wheel report per row of its distance: pixels divided by the
  terminal's row height on the page, lines as lines, a page as the terminal's rows.
- **WHL-002** The fraction of a row left over carries to the next turn in the same direction; a
  turn the other way starts from zero. One turn sends at most as many reports as the terminal has
  rows.
- **WHL-003** The reports are xterm's own (the program's encoding, the pointer's cell), and the
  turn does not scroll the page or reach xterm a second time.
- **WHL-004** Unchanged (SPEC-098 MSE-006, MSE-004): Ctrl+wheel and horizontal swipes stay with
  the browser; a modified wheel is xterm's as before; a read-only viewer or a program without
  mouse reporting gets xterm's own wheel handling.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] ADR, SPEC-098 and changes record wheel rows
- [x] Independent review in a fresh session, findings fixed test-first
- [ ] Merged into dev on green CI

## Acceptance criteria

- A wheel event of 5.5 rows of pixels sends 5 wheel-up reports. Ten of 0.2 rows each then send 2
  more, a 3-line event 3 more, and a turn back of 2.5 rows sends 2 wheel-down reports.
- The plain wheel's default action is prevented; Ctrl+wheel and a horizontal swipe are neither
  reported nor prevented (existing test).
- The full e2e suite, `scripts/dev ci` and `scripts/dev security` pass.

## Test plan

- `e2e/tests/terminal.e2e.ts` "a wheel turn reports one row per row of its distance (#863)": the
  events of the acceptance criteria dispatched over the task terminal of the mock; it counts the
  `ESC [ < 64 ;` and `ESC [ < 65 ;` reports sent, checks they share the pointer's cell, that two
  pages send one screen of rows and that a Shift or Alt wheel stays xterm's. It fails on `dev` (xterm sends one report per
  event and damps the small ones).
- The existing wheel, click, read-only and disconnected tests keep passing.

## Decisions

- **Report rows, not events, in the web terminal (ADR-0210).** The capture-phase wheel listener
  in `terminal-core.js` turns a plain wheel turn into rows and dispatches one line-mode wheel event
  per row to xterm, which reports each in the program's encoding at the pointer's cell. This is
  what Alacritty, kitty and iTerm2 send, and Claude Code's default of one row per report is then
  right. Programs with a larger step per report (vim: 3) scroll that much more, as in kitty.
  Alternatives:
  - Set `CLAUDE_CODE_SCROLL_SPEED=3` for agent sessions: only Claude Code benefits, and a trackpad
    stays damped (0.3×, then 3× as coarse).
  - Answer XTVERSION as `xterm.js(6.0.0)` so Claude Code applies its VS Code curve: Claude Code
    asks once at start, usually before anyone watches a task, and the server answers no queries.
  - Encode the reports ourselves with `term.input`: duplicates xterm's encodings and cell maths.
  - Patch the vendored xterm.js: the vendored files are unmodified npm builds (vendor README).

## Evidence

- **Reproduction.** Claude Code 2.1.292 in a PTY (`seq 1 300` in shell mode, then
  `ESC[<64;20;10M` × 10): the transcript moved one row per report and showed "Jump to bottom". In
  the mock UI, the real Claude Code byte stream replayed into the task terminal still sent wheel
  reports, one per event (the counts in Context).
- **Regression test first.** `npx e2e run tests/terminal.e2e.ts --grep 'one row per row'` at
  3073779d (test only): failed, "wheel-up reports: expected 3 to be 10". With the fix: passed.
- **Gate.** `scripts/dev ci` at 5b92659c: exit 0 (the review rounds after it touch only JS, the
  e2e test and docs).
- **e2e.** Full suite at a1be0553: 65 files passed, 1 skipped; 688 tests passed, 1 skipped. The
  terminal file at the final head (round-2 test assertions): 16 of 16 passed.
- **Security.** `yulius security-scan`: exit 2, not run. Vidoc answered HTTP 429: the instance key
  has used its 50,000,000 tokens. This is unrun, not passed; Yulius CI's `security` job repeats it.
- **Review.** Round 1 approved with minors (doc wording on other terminals and vim, missing cell,
  page-cap and modifier assertions, presence counted per row event, the mock's 3-line step,
  ADR-0079 back-reference): all fixed in a1be0553. Round 2 asked for exact modifier counts (xterm
  reports Alt+wheel as SGR 72 once and Shift+wheel not at all), a settle before the older test's
  `before`, the pointer's cell compared and two leftover "native terminal" phrases: all fixed.
- **Not tested.** Real hardware wheels and trackpads on macOS, Windows and Linux: the deltas were
  dispatched as Chrome reports them (40 px per macOS notch, 100 px elsewhere, 3 lines in Firefox).
  The fix was not tried on the live instance, because the sandbox browser cannot open Yulius.
