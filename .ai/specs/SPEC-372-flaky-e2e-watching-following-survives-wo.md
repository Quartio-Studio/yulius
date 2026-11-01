---
yulius-task: 9340bfbc-6810-43e8-802c-dbaf1d27e95a
status: in-progress
---

# SPEC-372: The terminal's first-show wait outlasts the app's own reveal limit (#949)

Status: in-progress
Date: 2026-11-01
Owner: Yulius task #355
Scope authorization: the issue-triage goal (task #354's handoff). Fix GitHub issue #949 in one pull
request into `dev` (`Fixes #949`). Create no new GitHub issue; comment progress and leftovers on
#949.

## Goal

`terminalShown` (`e2e/tests/support.ts`) waits as long as the app may take to show a task
terminal's first screen, so the tests that open a task view never fail for a loaded runner alone.

## Context

- **Symptom (#949).** In a full `npx e2e run` (2026-10-27, head 3ca362fb), `watching.e2e.ts`
  "following survives "Working…" hiding…" failed once at `terminalShown`: `expect.poll(...)`
  timed out after 5000 ms on `?mock=1#/s/studio/t/12`. The file alone passed 25/25.
- **Code path.** `terminalShown` polls for `.taskview .term` without `.term-pending` or
  `.term-placeholder`, with `expect.poll`'s default 5 s. The task view's terminal shows only
  after:
  1. `terminal.js` `loadCore()`: eight dynamic imports, the xterm stylesheet and the code font
     (the font wait is capped at `FONT_WAIT_MS`, 1.5 s; the rest is not). The idle-time preload
     (`app.js`, `requestIdleCallback`) is late on a busy runner too.
  2. `start()` (`terminal-core.js`): opens xterm and the socket, and starts the fallback
     `revealTimer` of `REVEAL_LIMIT_MS` (5 s) for a server that never draws.
  3. The hello's answer (state and screen) is drawn, then two frames (`maybeReveal`).
- **Root cause.** The helper's whole budget (5 s from its first poll) equals the app's fallback
  alone, and that fallback is counted from `start()`, after the core load. Whatever delays the
  core load or the hello's answer by more than the remaining time fails the wait: a loaded
  runner does it at random; a terminal whose server is silent does it every time. It is a test
  budget shorter than the app's contract, not a page defect.
- **No real race behind it.** `termsocket.js` has no connect or hello timeout that could drop the
  hello (its only timer is the reconnect delay), the mock answers every hello (`mock.js`
  `TermSession.receive`), and the app's fallback shows the terminal at most `REVEAL_LIMIT_MS`
  after `start()` whatever the server does. Nothing in #949's test depends on the screen's text.
- **Why the tests missed it.** Every caller opens a mock terminal that answers within
  milliseconds, so no test let the app's reveal take longer than the helper's budget.
- **Earlier work.** #354 (open) asks a person whether to allow one named, longer wait for first
  loads in general. This change does not decide that: the terminal's budget follows from the
  app's own reveal limit, which a 5 s wait cannot outlast at any load (ADR-0251).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| TSH-001 | `terminalShown` waits up to a named budget, `TERMINAL_SHOW_MS`, longer than the app's `REVEAL_LIMIT_MS` plus the terminal core's load on a starved runner, with a failure message that says the terminal never showed. | `e2e/tests/support.ts` |
| TSH-002 | A regression test holds the terminal core's load until the wait has started and holds the hello for good, so the terminal shows only by the app's fallback, more than 5 s after the wait began; it fails with the old wait and passes with the new one. | `e2e/tests/terminal.e2e.ts` |
| TSH-003 | No retries, sleeps or app changes; the budget bounds that one condition only. | diff |

## Plan

- [x] Spec with root cause of the terminalShown flake
- [ ] Regression test that holds the terminal past 5 s and fails
- [ ] Named first-show budget for terminalShown
- [ ] Suites using the terminal pass, docs and changes updated
- [ ] Independent review answered
- [ ] Merged into dev on green CI and issue closed

## Acceptance criteria

- The regression test fails on the unchanged helper with #949's signature (`expect.poll` timed
  out after 5000 ms in `terminalShown`) and passes with the change.
- `archive`, `reply`, `terminal`, `terminals` and `watching` e2e files pass.

## Test plan

- New `terminal.e2e.ts` case: `browser.route` holds `terminal-core.js` until `terminalShown` has
  started polling; `__yuliusHoldTerminal` holds the hello for good; the terminal shows by the
  app's fallback. Run it on the old helper (fails) and the new one (passes), several times.
- The five files that call `terminalShown`, and the full e2e suite.
- `scripts/dev ci` and `scripts/dev security` (scan disabled for this project).

## Decisions

- **A named budget derived from the app's contract (ADR-0251).** Options:
  - Keep 5 s and make the app reveal sooner: the fallback is for a silent server and would
    show an empty terminal over a slow one; changes the product for a test.
  - Wait in the test for the core load first, then 5 s: still shorter than the app's fallback,
    and a second condition to keep in step with `terminal.js`.
  - A named budget, `TERMINAL_SHOW_MS = 30_000`, as `MONACO_LOAD_MS` does for Monaco: chosen.
    It covers `REVEAL_LIMIT_MS` (5 s) plus the core load on a starved runner, bounds this one
    condition, and every caller (about 30, in 6 files) gets it.
- **The regression test drives the app's fallback, not a timer of its own.** It holds the hello
  for good, so the only way the terminal shows is `REVEAL_LIMIT_MS` after `start()`; holding the
  core until the wait has begun makes that later than 5 s after the wait's start every time,
  with no sleep in the test.
