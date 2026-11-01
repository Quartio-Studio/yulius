---
yulius-task: 9340bfbc-6810-43e8-802c-dbaf1d27e95a
status: done
---

# SPEC-372: The terminal's first-show wait outlasts the app's own reveal limit (#949)

Status: done
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
- [x] Regression test that holds the terminal past 5 s and fails
- [x] Named first-show budget for terminalShown
- [x] Suites using the terminal pass, docs and changes updated
- [x] Independent review answered
- [x] Full e2e suite passes at the final head
- [x] Merged into dev on green CI and issue closed

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
    condition, and every caller (24 calls in 5 files) gets it.
- **The regression test drives the app's fallback, not a timer of its own.** It holds the hello
  for good, so the only way the terminal shows is `REVEAL_LIMIT_MS` after `start()`; holding the
  core until the wait has begun makes that later than 5 s after the wait's start every time,
  with no sleep in the test.

## Evidence

All on `feat/355-flaky-e2e-watching-following-survives-wo` from dev d7346573, in the task sandbox.

- **Reproduction, old wait.** `npx e2e run tests/terminal.e2e.ts` at 5e0dc017 (regression test,
  unchanged helper): 29 passed, the new case failed with #949's signature, `ASSERTION_FAILED:
  expect.poll(...).toBe(...) timed out after 5000 ms, last: expected false to be true` at
  `tests/support.ts:122`.
- **Repeated, old against new.** A scratch file (not committed) ran the case with the new
  `terminalShown` and with the old 5 s poll, `--repeat-each 8`: old 0 of 8 passed (each
  `ASSERTION_FAILED` after ~6.5 s), new 8 of 8 passed (6.7–7.4 s each).
- **Callers.** `archive`, `reply`, `terminals`, `watching` and `terminal` with the fix (13892935):
  77 of 77 passed. After review round 1 (bfad08e3, hold count added) `terminal.e2e.ts`: 30 of 30.
- **Gate.** `python3 scripts/bootstrap.py && scripts/dev ci` at 13892935: exit 0, nextest 2949 of
  2949 passed (16 skipped). The round-1 commit after it changes only e2e TypeScript and Markdown;
  Yulius CI runs the gate on the final head.
- **Full e2e suite** (`npx e2e run` at bfad08e3, 40 min): 797 passed, 1 skipped (the agent test,
  no model key), 1 failed: `vidoc-switch.e2e.ts` "a refresh that overtakes the read after a change
  still shows the change" (`expect.poll` 5 s at line 348, CI page). That is #1002, fixed in
  parallel by task #357; it reads no terminal and does not call `terminalShown`, so it is outside
  this change. Every test calling `terminalShown` passed.
- **Security.** `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius";
  nothing was scanned (disabled, not passed).
- **Review round 1** (fresh session, approved 13892935): (1) minor, own: the case did not prove the
  core was held, so a renamed core would make it pass on a 5 s wait; fixed, it counts the route's
  holds. (2) nit, own: call counts (24 calls in 5 files); fixed. (3) minor, own: this Evidence
  section; added. (4) nit, own: the changes entry compared 5 s with 5 s; reworded. (5) nit,
  pre-existing: `terminal.e2e.ts` near "only keystrokes count as typing" says the terminal shows
  once its first state arrived, which the fallback does not guarantee; the mock answers within
  milliseconds, so no change.
- **Not tested.** A probe-controlled CPU share run: the held core and hello reproduce the timing
  deterministically instead, which the issue's acceptance allows.
