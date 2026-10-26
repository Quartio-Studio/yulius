---
yulius-task: cf4b899b-eff5-476a-8b19-a95aee7ead50
status: done
---

# SPEC-326: Scrolling past the top of a task's terminal opens its whole session

Status: done
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
- **Old tasks (reported during the task).** The person confirmed: "scrolling works for a new task
  but not for old ones". Runs survive updates frozen and thawed (SPEC-160 OPS-010), not restarted.
  So a Claude Code started before SPEC-310 (2026-10-25) still runs the classic main-screen renderer
  its boot canary chose, without `CLAUDE_CODE_NO_FLICKER`. It has no mouse reporting, and a viewer
  who opens the task gets only the visible screen: nothing above it to scroll. Old tasks also
  compacted more often than new ones.
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
  - xterm scrolls a normal buffer itself (the program has no mouse) and its viewport is already at
    the buffer's first line; or
  - elsewhere (the program has the mouse, or the alternate screen, where xterm turns the wheel into
    arrow keys for the program), the upper half of the screen is unchanged 400 ms after the turn.
    Turns closer together than that are one gesture. The check compares the screen at the gesture's
    start and at its last turn.

  A turn that moved the screen does not count. If the screen moves after the check showed the offer
  (a program slower than the check), the offer goes. A blank screen, and a task that never started,
  offer nothing. So does a scrollback whose first screenful is blank, at its top (ADR-0220
  Consequences). Neither do other terminals (Shells, sign-in terminals).
- **HIS-002 The way on is offered.** Reaching the top shows a button over the top of the task's
  terminal, below its banner, its connection notice, and the attention bar or held card:
  "Earlier messages: show the whole session". It hides after 8 s, on a wheel turn down, on a key
  in the terminal, or when clicked. Enter or Space on the button opens the session. It takes
  neither the keyboard nor the PTY size.
- **HIS-003 The whole session.** The button opens the History view in the terminal's place, without
  closing the terminal: its socket, size and screen stay as they were.
  - The view reads the task's session from line 0 (`from=0`) in pages of 500 entries to its end. It then
    follows the session live, as a watcher's view does (SPEC-179 WAT-003), and starts at the
    bottom. Scrolling up reaches the session's first message.
  - A new session (a continue that started another) is read again from line 0.
  - The view says what it is: "The whole session, from its first message. Read-only." It has a
    "Back to the terminal" button. Escape anywhere in the view goes back too, and the keyboard
    returns to the terminal. So do the attention bar's Reply, a reply it sends and a queued
    message's Dismiss, because each answer goes into the terminal.
- **HIS-004 Unchanged.** The wheel's reports to the program (SPEC-280), the selection keys, and the
  watchers' view of someone else's task (SPEC-179) all work as before. So does a terminal without a
  task.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] A History view reads a task's session from its first line
- [x] Reaching the top of a task terminal offers the whole session
- [x] ADR, docs and changes record the history view
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Old runs on the classic renderer offer the whole session
- [x] Final head verified: review, gate and full e2e suite
- [x] Merged into dev on green CI

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
- `e2e/tests/terminal.e2e.ts`, "an old run on the classic renderer, with nothing above its screen,
  offers the whole session at once (#911)": the mock draws the main screen without mouse
  reporting (`?classic=1`). The first wheel turn up shows the offer and sends no wheel report.
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
- **Old runs.** At 9c9a93cd, with `terminal-core.js`, `taskview.js` and `watchview.js` taken from
  `origin/dev`, both #911 tests failed (2 failed, 16 passed). With the fix, `tests/terminal.e2e.ts`
  passed 18 of 18.
- **Review round 1** (fresh session, at 9c9a93cd): APPROVE with 6 minors, 3 nits and test gaps, all
  own code. The tests at feffccd7 failed at 6db5da8c for the reasons the review named:
  - the late program: "no offer while the program scrolls", true;
  - Space on the offer: History never opened;
  - the second page: `from=500&session=b1f0-12` without `limit=500`.

  The scrollback test only covered a gap. With the fixes at c8adfd3d, the terminal, watching and
  reply files passed 46 of 46. Answers:
  1. the late program takes the offer back (`onWriteParsed`);
  2. the button's own keys open it;
  3. pages are 500 entries;
  4. HIS-001 reworded;
  5. the offer lives in the terminal view, below its banner and connection notice;
  6. the down-turn step waits for the screen;
  7. focus, connection, paging, session change and scrollback are tested;
  8. Escape anywhere in the view goes back, and focus follows a control change.
- **Review round 2** (fresh session, at c8adfd3d): REQUEST CHANGES for one major. The attention
  bar's Reply focused the hidden terminal while the whole session showed. Also 2 minors: the offer
  sat under a tall held card, and a task that never ran was offered its session. 4 nits. The
  tests added after it failed at c8adfd3d (History still visible after Reply; button top 129 px
  over a card ending at 225 px; offer shown for product #7). With the fixes, the terminal,
  watching, reply and queued-messages files passed 76 of 76. Answers:
  - Reply, Send and Dismiss close the History view first.
  - The task view measures the floating cards (`--term-overlay-bottom`).
  - A blank screen, or a task that never started, offers nothing.
  - Test steps hide the offer with a key before a "next turn offers" check.
  - `deltaY` 0 is no turn.
  - ADR-0220 states the cost of reading page by page.
- **Review round 3** (fresh session, at b219974a): APPROVE with 2 minors and 3 nits, all own code.
  The tests at 1e52704c failed at b219974a, for the right reasons:
  - a reply sent while offline left the History view open over the terminal's notice;
  - the blank-scrollback case got no offer at its top.

  At b007e901, the terminal, watching, reply and queued-messages files passed 79 of 79. Answers:
  - a reply closes the History view before it is sent;
  - a blank-screen test was added, and the never-ran test waits for the screen's text (it
    exercises `when`);
  - queue Dismiss is tested;
  - the ResizeObserver is used without a `typeof` check, and writes only a changed value.

  The blank first screenful nit was not shipped: the mock could not build that state (xterm kept no
  blank first screenful), so its scan stayed untested and was removed. ADR-0220 records the limit.
- **Verification** (fresh session, at b007e901): APPROVE with 2 nits. `screenBlank` duplicated
  `screenEmpty`, and is now replaced by it. This Evidence entry and the HIS-001 limit answer the
  other.
- **e2e.** The full suite at b007e901: 66 files passed, 1 skipped; 726 tests passed, 1 skipped.
  After the `screenEmpty` change (ef23664b), `tests/terminal.e2e.ts` passed 27 of 27.
- **Gate.** `scripts/dev ci` at ef23664b: exit 0, 2622 tests run, 2622 passed, 16 skipped. It also
  passed at b219974a with the same counts. The commits after ef23664b change only prose.
- **Security.** `yulius security-scan` at fe124b11: exit 2, not run. Vidoc answered HTTP 429: the
  instance key has used its 50,000,000 tokens. This is unrun, not passed; Yulius CI's `security`
  job repeats it.
- **Not tested.**
  - The fix was not tried on the live instance or on the person's old tasks. The sandbox browser
    cannot open Yulius, and other projects' runs are not reachable from here. Old runs were
    reproduced with the mock's classic renderer (`?classic=1`).
  - The pre-compaction cut-off was shown with the real Claude Code 2.1.292 in a PTY. The e2e tests
    drive the mock agent.
  - Real wheels and trackpads were not used: the tests dispatch wheel events, plus the browser's
    own wheel for xterm's scrollback.
  - Phones: a swipe is no wheel turn, so a phone is not offered the whole session (ADR-0220).
