# 0210. A wheel turn reaches a full-screen program as one report per row

## Status

Accepted. Specified in [SPEC-280](../../.ai/specs/SPEC-280-i-can-t-scroll-up-in-claude-conversation.md).
Refines [ADR-0079](0079-the-terminal-mouse-is-the-programs.md): the wheel stays the program's,
with as many reports as the turn has rows.

## Context

Claude Code runs full screen with mouse reporting and moves its transcript one row per wheel
report. It moves 3 only in terminals it recognises as xterm.js (`TERM_PROGRAM=vscode`, or an
XTVERSION answer), which Yulius's terminal is not. xterm.js 6.0 sends at most one report per
browser wheel event and scales pixel deltas under 50 px down to 0.3×. A mouse notch therefore
scrolled one row of a long conversation, and a trackpad swipe about a third of its distance. The
person found they could not scroll up (task #266, #863).

## Decision

While the program has the mouse, the terminal's capture-phase wheel listener turns a plain wheel
turn (no modifier, a vertical delta) into rows: pixels over the row height on the page, lines as
lines, a page as the terminal's rows. A fraction carries to the next turn in the same direction,
and one turn sends at most a screen of rows. It cancels the turn and dispatches one line-mode wheel
event per row to xterm, which reports each in the program's encoding at the pointer's cell. Native
terminals send the same: one report per row scrolled.

## Alternatives

- `CLAUDE_CODE_SCROLL_SPEED=3` in agent sessions: only Claude Code benefits, and the trackpad stays
  damped and three times as coarse.
- Answer XTVERSION as xterm.js so Claude Code applies its VS Code curve: Claude Code asks once at
  start, usually before anyone watches the task, and Yulius answers no queries itself.
- Encode the reports in Yulius with `term.input`: a second copy of xterm's encodings and cell maths.
- Patch the vendored xterm.js: the vendored files are unmodified npm builds.

## Consequences

- The wheel and the trackpad scroll Claude Code, vim, `less` and other mouse-reporting programs as
  far as they scroll a page. A fast swipe sends a burst of small input frames (at most a screen's
  rows per event).
- Modified wheels, Ctrl+wheel (pinch-zoom), horizontal swipes, read-only viewers and programs
  without mouse reporting keep xterm's or the browser's handling.
- If Claude Code starts recognising this terminal as xterm.js, it would multiply the rows by 3;
  the e2e test pins the reports, and the speed is then set back with `CLAUDE_CODE_SCROLL_SPEED=1`.
