---
yulius-task: cf4b899b-eff5-476a-8b19-a95aee7ead50
status: in-progress
---

# SPEC-326: Scrolling past the top of a task's terminal opens its whole session

Status: in-progress
Date: 2026-10-26
Owner: task #310
Scope authorization: task #310: "I can't scroll up terminals with claude. For every task I want to
be able to scroll to the very beggining. Fix". Fixes #911. Builds on
[SPEC-280](SPEC-280-i-can-t-scroll-up-in-claude-conversation.md) (the wheel reaches Claude Code),
[SPEC-310](SPEC-310-i-can-t-scroll-up-fix.md) (Claude Code always draws full screen) and
[SPEC-179](SPEC-179-ui-problem-when-someone-else-has-a-contr.md) (a task's live transcript).

## Goal

From every task's terminal, the person can scroll to the very first message of the task's session,
however long it ran and however often it was compacted or resumed.

## Context

Confirmed against `dev` at 61eddc83. The running server (24916d9c) already has the SPEC-280 and
SPEC-310 fixes.

- **Symptom.** Scrolling up in a task terminal ends before the session's start, and the person has no
  way to read further back.
- **Root cause.** The wheel reaches Claude Code (SPEC-280), and Claude Code draws full screen
  (SPEC-310). But Claude Code shows a session only from its last compaction. Reproduced with the
  instance's Claude Code 2.1.292 in a PTY, with `CLAUDE_CODE_NO_FLICKER=1` as Yulius launches it:
  - A fresh session (`! seq 1 2000`) scrolled to line 1 with SGR wheel reports. So did the same
    session resumed (`--resume`).
  - A resumed 8 MB task transcript without a compaction scrolled to its first message.
  - A resumed 11.7 MB task transcript with one compaction stopped at
    "❯ /compact · Compacted (ctrl+o to see full summary)". 3,400 more wheel-up reports changed
    nothing. In its ctrl+o transcript, ctrl+Home and `g` stop at "Compact summary".

  Everything before the compaction stays in the transcript file
  (`<config>/projects/<cwd>/<session>.jsonl`), but nothing in the task view reaches it. Compactions
  are common: in this project, tasks from the last two days have 1 to 6 each. Yulius resumes
  sessions after every update and usage limit, and a resumed session shows nothing before its last
  compaction.
- **Same cause elsewhere.** A viewer whose terminal shows a screen without scrollback cannot scroll
  up at all. This happens when the program draws the alternate screen without mouse reporting, or
  after a reload of a classic-renderer screen (the hub's model keeps no scrollback, SPEC-310). A
  normal-buffer terminal ends at xterm's 5,000 lines of scrollback.
- **Why the tests missed it.** The mock agent's transcript is always on its screen. No test covers
  the end of what the terminal can scroll to.
- **What exists.** `GET /api/tasks/{tid}/transcript` (SPEC-179 WAT-001) reads a task's latest session
  from any line, redacted. `watchview.js` prints that session as the Sessions view prints it, live,
  for people watching someone else's task. It starts from the tail (200 entries) and links to
  Sessions for the rest.

## Requirements

- **HIS-001 The top is noticed.** In a task's terminal, a wheel turn up (no Ctrl, a vertical delta
  up) reaches the top when either:
  - the program has the mouse and the upper half of its screen is unchanged 400 ms after the turn's
    last report (the program did not scroll); or
  - xterm scrolls itself and its viewport is at the buffer's first line. On the alternate screen
    that is always true, because there is no scrollback.

  A turn that moved the screen does not count. Other terminals (Shells, sign-in terminals) get no
  callback.
- **HIS-002 The way on is offered.** Reaching the top shows a button over the top of the task's
  terminal: "Earlier messages: show the whole session". It hides after 8 s, on a wheel turn down,
  on a key, or when clicked. It takes neither the keyboard nor the PTY size.
- **HIS-003 The whole session.** The button opens the History view in the terminal's place, without
  closing the terminal: its socket, size and screen stay as they were.
  - The view reads the task's session from line 0 (`from=0`) page by page to its end. It then
    follows the session live, as a watcher's view does (SPEC-179 WAT-003), and starts at the
    bottom. Scrolling up reaches the session's first message.
  - A new session (a continue that started another) is read again from line 0.
  - The view says what it is: "The whole session, from its first message. Read-only." It has a
    "Back to the terminal" button. Escape in the view goes back too, then focus returns to the
    terminal.
- **HIS-004 Unchanged.** The wheel's reports to the program (SPEC-280), the selection keys, and the
  watchers' view of someone else's task (SPEC-179) all work as before. So does a terminal without a
  task.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] A History view reads a task's session from its first line
- [x] Reaching the top of a task terminal offers the whole session
- [x] ADR, docs and changes record the history view
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Merged into dev on green CI

## Acceptance criteria

- In the mock's task #12, wheel turns up past the top of the agent's transcript show the button. A
  turn that still scrolls does not show it.
- Clicking the button shows the History view with the session's first entry. The first read asks
  for `from=0`, and the terminal is still in the page, hidden and connected.
- "Back to the terminal" and Escape bring the terminal back.
- The full e2e suite, `scripts/dev ci` and `scripts/dev security` pass.

## Test plan

- `e2e/tests/terminal.e2e.ts`, "a wheel turn past the top offers the whole session from its first
  message (#911)": the acceptance criteria above, over the mock's task terminal. It fails on `dev`
  because there is no button.
- The existing wheel, read-only and watching tests keep passing (HIS-004).

## Decisions

- **Yulius shows the whole session from its transcript; it does not change what Claude Code keeps
  (ADR-0220).** Claude Code cannot show anything before a compaction once it is resumed, and no
  setting changes that. Yulius already serves the session's transcript, redacted, from any line,
  and prints it as the Sessions view does. Alternatives:
  - Turn auto-compaction off: a full context stops the task (SPEC-060), and the history would still
    end at a manual `/compact`.
  - Keep a scrollback in the hub and replay it: Claude Code draws full screen, so there is no
    scrollback to keep, and a resume starts at the compaction anyway.
  - A permanent History button only: easy to miss where the person is looking, which is the top of
    the terminal they are scrolling. The button appears exactly there and then.
  - Join the transcript above the terminal in one scroller: the transcript and Claude Code's screen
    show the same messages twice, at different widths, and the wheel would belong to two scrollers.
- **The top is noticed from the screen, not from Claude Code's words.** A wheel turn that changes
  nothing in the upper half of the screen means the program is at its top, whatever it draws there.
  Matching Claude Code's texts ("Compact summary", its banner) breaks with its next release. The
  lower half is left out, because the spinner and the prompt change there while the agent works.
- **Starts at the bottom, reads from line 0.** The person arrives from scrolling up, so the view
  opens at the latest message and they keep scrolling up. The whole file is read up front, page by
  page (2 MiB of text at most per page), so the first message is there when they reach it.

## Evidence

- **Reproduction.** The PTY runs in Context used a copy of this sandbox's config dir and the
  instance's Claude Code 2.1.292 (`pexpect` and `pyte`, 100×30 and 120×40). A fresh session with
  `! seq 1 2000`: 3,400 wheel-up reports reached "⎿ 1". The same session resumed did too. A resumed
  8 MB task transcript reached its first message, and ctrl+Home jumped there. A resumed 11.7 MB
  task transcript with one compaction stopped at "❯ /compact · Compacted"; ctrl+o, then ctrl+Home
  and `g`, stopped at "Compact summary".
- **Regression test first.** `npx e2e run tests/terminal.e2e.ts` at 71d21bf0 (test only) failed:
  `.taskview .term-history-offer`: "observed: no node". The program had scrolled first, so the
  failure is the missing offer. With the fix (6a771a8b), `tests/terminal.e2e.ts` and
  `tests/watching.e2e.ts` passed: 42 of 42.
- **Security.** `yulius security-scan` at fe124b11: exit 2, not run. Vidoc answered HTTP 429: the
  instance key has used its 50,000,000 tokens. This is unrun, not passed; Yulius CI's `security`
  job repeats it.
