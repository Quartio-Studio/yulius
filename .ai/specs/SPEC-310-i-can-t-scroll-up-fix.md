---
yulius-task: 1f8c824e-7ccb-45d2-826c-9a7aaede0b33
status: done
---

# SPEC-310: Every Claude Code run Yulius starts draws full screen

Status: done
Date: 2026-10-25
Owner: task #296
Scope authorization: task #296: "i can't scroll up. Fix." (a screenshot of task #32's terminal
in the quartio-v2 project). Fixes #898. Builds on [SPEC-280](SPEC-280-i-can-t-scroll-up-in-claude-conversation.md)
(ADR-0210) and SPEC-098 (ADR-0079), which assume Claude Code draws full screen.

## Goal

In every task terminal and the Superagent (the Cockpit), Claude Code draws full screen, so the wheel
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
- **Same cause elsewhere.** The Superagent (the Cockpit, `plan_cockpit`: a person's own Claude
  Code) shares the same canary in the person's config dir. The commit-message proposer
  (`claude -p`, `TERM=dumb`) and `claude setup-token` sign-ins draw no conversation screen. A
  `claude` a person types in a Shell is not a launch Yulius makes and is left as it is: whatever it
  records in the canary cannot turn off the forced launches.

## Requirements

- **FSR-001** Every interactive Claude Code launch plan (task runs, `plan_launch`, and the
  Superagent, `plan_cockpit`) sets `CLAUDE_CODE_NO_FLICKER=1` once. Claude Code then starts on its
  full-screen renderer whatever `fullscreenAutoDisabled`, its feature gate or a saved `tui`
  setting says. A `/tui default` typed in a session still switches that session (Claude Code
  relaunches itself without the variable) until its next start.
- **FSR-002** Unchanged: the wheel, click and late-viewer behaviour of SPEC-280 and SPEC-098; the
  non-interactive proposer's environment.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Fix spec and the regression test that fails
- [x] Every Claude Code run Yulius starts draws full screen
- [x] ADR, docs and changes record the forced renderer
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Merged into dev on green CI

## Acceptance criteria

- `plan_launch`'s environment is exactly the earlier one plus `CLAUDE_CODE_NO_FLICKER=1`, and
  `plan_cockpit`'s holds it exactly once, set to `1`.
- With a config dir whose `fullscreenAutoDisabled` is set, Claude Code 2.1.292 started with the
  planned variable enters the alternate screen and turns on mouse reporting (manual reproduction
  above).
- `scripts/dev ci` and `scripts/dev security` pass.

## Test plan

- `crates/yulius-harness/tests/launch.rs`: `claude_new_session_argv_snapshot_and_files` pins the
  task run's environment, and `a_cockpit_is_plain_claude_code_with_the_token_only_in_its_environment`
  checks the Superagent's (one entry, `1`). Both fail on `dev`.
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

- **Reproduction.** The PTY runs of Context, with a copy of this sandbox's config dir and the
  instance's Claude Code 2.1.292. Two seeded sibling starts set `fullscreenAutoDisabled {strikes:
  2}`, and later starts showed no `ESC[?1049h` and no mouse modes. With `CLAUDE_CODE_NO_FLICKER=1`
  the same config entered the alternate screen with mouse reporting (1000, 1002, 1003, 1006), and
  sibling starts recorded no strike. The 2.1.292 code checks the variable before the crash
  auto-off, the feature gate and the `tui` setting. Only a screen reader,
  `CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN` or `CLAUDE_CODE_NO_FLICKER=0` turn it off before it. A probe of mine had left
  a pending canary entry (pid 693) in this sandbox's real config, and I removed it.
- **Regression test first.** `cargo test -p yulius-harness --test launch` at d4a1f7fa (test
  only) failed in `claude_new_session_argv_snapshot_and_files` (env without
  `CLAUDE_CODE_NO_FLICKER`) and in `a_cockpit_is_plain_claude_code_with_the_token_only_in_its_environment`.
  With the fix (7b29e374) all 13 passed. A mutation at 0dcf6794 (the Cockpit's entry removed)
  failed the tightened Cockpit assertion.
- **Gate.** `scripts/dev ci` at 8574e88e: exit 0, 2612 tests passed, 16 skipped. Run again at
  40b9842c (after the dev merge; the commits after it change only prose).
- **Security.** `yulius security-scan`: exit 2, not run. Vidoc answered HTTP 429: the instance key
  has used its 50,000,000 tokens. This is unrun, not passed; Yulius CI's `security` job repeats it.
- **Review.** Round 1 (fresh session, om-code-review) approved 8574e88e with minors, all answered
  in 0dcf6794:
  1. The Superagent is the Cockpit (`plan_cockpit`), not a task run: wording fixed everywhere.
  2. `/tui default` typed in a session still switches it, because Claude Code relaunches itself
     without the variable: FSR-001 and ADR-0218 reworded.
  3. The spec was unfinished: completed here.
  4. A `claude` typed in a Shell gets no variable: left as it is, with the reason in Context and
     ADR-0218.
  5. The Cockpit assertion now requires exactly one entry, set to `1`.
  6. Pre-existing nit, outside this change: the comment at `supervisor.rs` (~1342) says Yulius's
     environment wins over project variables, but the plan's variables come first. It is harmless
     because project variables cannot use the `CLAUDE_` prefix. The change does not touch that
     ordering.
- **Not tested.** The fix was not tried on the live instance or on quartio-v2's task #32: its
  sandbox is not reachable from here. Its next Claude Code start takes the variable. No test
  starts a signed-in Claude Code, so a later CLI that drops the variable would not fail CI
  (ADR-0218 Consequences).
