# 0218. Claude Code runs always draw full screen

## Status

Accepted. Specified in [SPEC-310](../../.ai/specs/SPEC-310-i-can-t-scroll-up-fix.md). Keeps true
the assumption of [ADR-0079](0079-the-terminal-mouse-is-the-programs.md) and
[ADR-0210](0210-a-wheel-turn-reaches-the-program-as-one-report-per-row.md) that Claude Code draws
full screen and has the mouse.

## Context

Yulius's web terminal relies on Claude Code's full-screen renderer. The wheel reaches it as mouse
reports, and a viewer who opens a task later gets the hub's screen, which keeps no scrollback.
Claude Code 2.1.292 chooses the renderer at each start, from a feature gate, a `/tui` setting and
a boot canary. A start records `fullscreenBootPending[<pid>]` in `<CLAUDE_CONFIG_DIR>/.claude.json`
and removes it 10 s after its first frame. At the next start, a pending entry whose pid is gone or
equals its own counts as a failed start. Two failed starts for one version set
`fullscreenAutoDisabled`, and every later start uses the classic main-screen renderer, with no
mouse reporting.

Tasks of a project share one config dir, and every contained run's `claude` is pid 2 in its own
bwrap PID namespace. A task that started within 10 s of a sibling's first frame took the sibling's
entry for its own failed start, and so did a run started after one that was stopped early. Two such
starts turned full screen off for the whole project. Its terminals then showed only the visible
screen, and nothing could be scrolled up (task #296, #898).

## Decision

Every interactive Claude Code launch, meaning task runs (which include the Superagent) and the
Cockpit, sets `CLAUDE_CODE_NO_FLICKER=1` (`FULL_SCREEN_ENV` in `yulius-harness`). Claude Code
documents this variable as the override. It takes precedence over the recorded auto-disable, the
feature gate and the `tui` setting, and with it Claude Code arms no canary. A screen reader or an
explicit `CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN` still turns full screen off.

## Alternatives

- **Clear the canary's keys before each launch.** `claude_config` would remove
  `fullscreenAutoDisabled`, `fullscreenBootStrikes` and `fullscreenBootPending`. Parallel starts
  would still trip it between two launches, the renderer would still follow the feature gate, and
  the keys are private and unversioned.
- **A config dir per task.** This loses the sign-in, trust, history and plugins that a project's
  tasks share today.
- **A scrollback in the hub for the classic renderer.** That renderer turns off mouse reporting
  and clears the scrollback when it redraws (`ESC[3J`), and every snapshot and final-screen path
  would change.

## Consequences

- Every Yulius-started Claude Code session draws full screen, and the wheel scrolls it for every
  viewer. A project whose config already recorded `fullscreenAutoDisabled` recovers at its next
  start, with no edit to the config.
- A person cannot choose the classic renderer for Yulius sessions with `/tui default`. Yulius's
  terminal does not support it.
- If a later Claude Code renames or drops the variable, the launch tests still pass. The manual PTY
  check in SPEC-310 then has to be repeated for that version.
