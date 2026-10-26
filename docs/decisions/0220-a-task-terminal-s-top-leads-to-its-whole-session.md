# 0220. A task terminal's top leads to its whole session

## Status

Accepted. Specified in [SPEC-326](../../.ai/specs/SPEC-326-i-can-t-scroll-up-terminals-with-claude.md).
Builds on [ADR-0210](0210-a-wheel-turn-reaches-the-program-as-one-report-per-row.md) (the wheel
reaches Claude Code) and [ADR-0218](0218-claude-code-runs-always-draw-full-screen.md) (Claude Code
draws full screen). Reuses the live transcript of
[SPEC-179](../../.ai/specs/SPEC-179-ui-problem-when-someone-else-has-a-contr.md).

## Context

People want to scroll every task's terminal back to the session's first message. The wheel reaches
Claude Code, and Claude Code draws full screen, but it shows a session only from its last
compaction. Claude Code 2.1.292, resumed (`--resume`) on a compacted task transcript, stops at
"/compact · Compacted". Its ctrl+o transcript stops at the compact summary, and no setting changes
that. Tasks compact often (1 to 6 times in this project's recent tasks), and Yulius resumes sessions
after every update and usage limit. A terminal without scrollback (the alternate screen without
mouse reporting, or a reloaded classic-renderer screen) cannot scroll up at all. The earlier
conversation is still in the session's transcript, which Yulius already serves, redacted, from any
line (`GET /api/tasks/{tid}/transcript`) and prints as the Sessions view does (`watchview.js`).

## Decision

A wheel turn up that reaches the top of a task's terminal offers the whole session.

- **When the top is reached.** xterm scrolls a normal buffer itself, so there the top is its first
  line. Elsewhere the program moves the screen, and the top is a turn that left the upper half of
  the screen unchanged 400 ms later. Turns closer together than that are one gesture. The check
  compares the screen at the gesture's start and at its last turn, because the program's answer
  arrives a round trip after each turn. If the screen moves after the check showed the button, the
  button goes: the program answered later than the check.
- **What is offered.** A button over the top of the terminal: "Earlier messages: show the whole
  session". The terminal view draws it (`topOffer` in `terminal-core.js`), so it sits below the
  banner and the connection notice. It hides after 8 s, on a turn down, on a key in the terminal
  or when clicked.
- **The History view.** It opens in the terminal's place. It is the watchers' live transcript view
  in a `whole` mode: it reads from line 0, page by page, to the end and then follows the session.
  It starts at the bottom, and its "Back to the terminal" button or Escape returns. The terminal
  stays connected, hidden, underneath.

## Alternatives

- **Turn auto-compaction off.** A full context then stops the task (SPEC-060), and a manual
  `/compact` would still cut the history.
- **Keep a scrollback in the hub.** Claude Code draws full screen, so there is no scrollback to
  keep, and a resume starts at the compaction anyway.
- **Only a permanent History button.** The person is looking at the top of the terminal they are
  scrolling, and a button elsewhere is easy to miss. It also adds a control that most moments do
  not need.
- **Join the transcript above the terminal in one scroller.** The same messages would show twice,
  at different widths, and the wheel would belong to two scrollers.
- **Recognise Claude Code's top from its text** ("Compact summary", its banner). This breaks with
  its next release and fits no other program.

## Consequences

- From every task's terminal, the person reaches the session's first message, compacted, resumed or
  not. The same applies to read-only viewers and to programs other than Claude Code. It also
  applies to old runs: they were started before ADR-0218 and kept the classic renderer through the
  updates that froze and thawed them, and their viewers get only the visible screen. For those
  runs the first turn up offers it.
- The view shows the task's latest Claude Code session. Sessions the task had before (a `/clear`, or
  a continue in a fresh session) stay in Sessions.
- A gesture that ends within one round trip of reaching the top may offer nothing, because the
  screen was still moving. The next turn offers it.
- The offer needs a wheel or a trackpad. A touch screen's swipe is no wheel turn, so phones are
  not offered it.
- A program that ignores the wheel (a dialog, a pager without mouse support) also gets the offer.
  The offer is harmless and goes away by itself.
- The whole transcript is read up front, at most 500 entries or 2 MiB of text per request. A
  multi-megabyte session takes a few requests before its first message is there. Each page is read
  from the transcript's start, because `read_page` scans earlier lines for the dotenv redaction.
  An 11.7 MB session read as about 12 pages therefore scans about 140 MB on the server, once per
  opening. A byte-offset cursor would remove that if it matters.
- Answers to the agent (the attention bar's Reply, a reply it sends, a queued message's Dismiss)
  close the History view first, because they go into the terminal.
