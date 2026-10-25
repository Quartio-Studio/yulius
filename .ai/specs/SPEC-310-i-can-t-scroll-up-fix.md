---
yulius-task: 1f8c824e-7ccb-45d2-826c-9a7aaede0b33
status: in-progress
---

# SPEC-310: Every Claude Code run Yulius starts draws full screen

Status: in-progress
Date: 2026-10-25
Owner: task #296
Scope authorization: task #296: "i can't scroll up. Fix." (a screenshot of task #32's terminal
in the quartio-v2 project). Fixes #898. Builds on [SPEC-280](SPEC-280-i-can-t-scroll-up-in-claude-conversation.md)
(ADR-0210) and SPEC-098 (ADR-0079), which assume Claude Code draws full screen.

## Goal

In every task terminal, Superagent and Cockpit, Claude Code draws full screen, so the wheel
scrolls its conversation for every viewer (SPEC-280). This holds whatever Claude Code recorded
about earlier starts in the project's shared config.

## Context

Confirmed with Claude Code 2.1.292 (the instance's CLI) against `dev` at f26b980b. The running
server (10e5de4e) already has the SPEC-280 wheel fix.

- **Symptom.** The terminal of task #32 (project quartio-v2) cannot be scrolled up at all. That
  is not the slow scrolling of #863.
- **Root cause.** Claude Code's full-screen renderer runs a boot canary. A start records
  `fullscreenBootPending[<pid>]` in `<CLAUDE_CONFIG_DIR>/.claude.json`, and the entry is removed
  10 s after the first frame. The next start counts a pending entry as a failed start when that
  pid is gone **or equals its own pid**. Two strikes for one version set `fullscreenAutoDisabled`,
  and every later start uses the classic main-screen renderer ("fullscreen disabled: turned off on
  this machine after repeated failed starts"). In Yulius every task of a project shares one config
  dir (the sandbox home), and every contained run's `claude` is pid 2 in its own bwrap PID
  namespace (`--unshare-pid`). So a task started within 10 s of a sibling's first frame records a
  strike, and so does a run that was stopped early. Two parallel starts disable full screen for the
  project until Claude Code's next version.
- **What the person sees.** In the main-screen renderer, Claude Code turns off mouse reporting and
  writes its conversation into the normal buffer. The hub's screen model keeps no scrollback (a
  vt100 parser with 0 rows of it), and a snapshot is the visible screen. A viewer who opens or
  reloads the task, or reconnects after lagging, therefore has nothing above the screen to scroll.
  The wheel is xterm's own and has no history to move through.
- **Reproduction.** In a copy of this sandbox's config dir, each child wrote a pending entry under
  its own pid (what a sibling in another PID namespace leaves) and then exec'd `claude` in a PTY.
  - Start 1: `fullscreenBootStrikes {count: 1}`, no `ESC[?1049h`.
  - Start 2: `fullscreenAutoDisabled {strikes: 2}`.
  - Starts 3 and 4, with no sibling: no alternate screen, no mouse reporting.
  - A fresh copy started alone: alternate screen and mouse reporting (1000, 1002, 1003, 1006).
  - The disabled copy with `CLAUDE_CODE_NO_FLICKER=1`: alternate screen and mouse reporting. Two
    sibling starts with it record no strike, because the canary is not armed.
- **Why the tests missed it.** Every terminal test drives a mock agent that always draws full
  screen (`--tui`). No test covers Claude Code's own renderer choice, which depends on its config
  state and a remote feature gate.
- **Same cause elsewhere.** The Cockpit (a person's own Claude Code) shares the same canary in the
  person's config dir. Superagent runs are task runs (`plan_launch`). The commit-message proposer
  (`claude -p`, `TERM=dumb`) does not render a TUI.

## Requirements

- **FSR-001** Every interactive Claude Code launch plan (task runs, which include the Superagent,
  and the Cockpit) sets `CLAUDE_CODE_NO_FLICKER=1`. Claude Code then uses its full-screen renderer
  whatever `fullscreenAutoDisabled`, its feature gate or a `/tui` choice says.
- **FSR-002** Unchanged: the wheel, click and late-viewer behaviour of SPEC-280 and SPEC-098; the
  non-interactive proposer's environment.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Fix spec and the regression test that fails
- [ ] Every Claude Code run Yulius starts draws full screen
- [ ] ADR, docs and changes record the forced renderer
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Merged into dev on green CI

## Acceptance criteria

- `plan_launch`'s environment is exactly the earlier one plus `CLAUDE_CODE_NO_FLICKER=1`, and
  `plan_cockpit`'s contains it.
- With a config dir whose `fullscreenAutoDisabled` is set, Claude Code 2.1.292 started with the
  planned variable enters the alternate screen and turns on mouse reporting (manual reproduction
  above).
- `scripts/dev ci` and `scripts/dev security` pass.

## Test plan

- `crates/yulius-harness/tests/launch.rs`: `claude_new_session_argv_snapshot_and_files` pins the
  task run's environment, and `a_cockpit_is_plain_claude_code_with_the_token_only_in_its_environment`
  checks the Cockpit's. Both fail on `dev`.
- The manual PTY reproduction above, against the real CLI, shows the variable overrides the
  recorded auto-disable. A test cannot run it: it needs a signed-in Claude Code.

## Decisions

- **Force the full-screen renderer with `CLAUDE_CODE_NO_FLICKER=1` (ADR-0218).** Claude Code
  documents it as the override ("/tui fullscreen or CLAUDE_CODE_NO_FLICKER=1 to override"), and
  with it Claude Code arms no canary. It also covers the remote feature gate and a person's
  `/tui default`. Yulius's terminal needs full screen: without it, late viewers have no
  scrollback. Alternatives:
  - Clear `fullscreenAutoDisabled`, `fullscreenBootStrikes` and `fullscreenBootPending` in
    `claude_config` before each launch. Parallel starts would still trip the canary between two
    launches. The renderer would still depend on the feature gate. It edits private, unversioned
    keys.
  - Give each task its own config dir. That loses the shared sign-in, trust, history and plugins
    that tasks of a project share today, which is a much larger change.
  - Keep a scrollback in the hub and send it to late viewers. The classic renderer still turns
    off mouse reporting and clears the scrollback on its redraws (`ESC[3J`). Every snapshot and
    final-screen path would change.

## Evidence

To be completed.
