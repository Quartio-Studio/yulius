# 0251. The terminal's first-show wait outlasts the app's reveal limit

## Status

Accepted. Specified in
[SPEC-372](../../.ai/specs/SPEC-372-flaky-e2e-watching-following-survives-wo.md); task #355, issue
#949. Amends [ADR-0122](0122-the-e2e-server-keeps-up-with-a-page-under-load.md), whose only named
budget was Monaco's cold load (`MONACO_LOAD_MS`): the terminal's first screen gets one too. Leaves
#354's general question about first-load budgets open.

## Context

The e2e helper `terminalShown` waited for a task terminal's first screen with an assertion's
default 5 s. The app shows that terminal once its core has loaded (`terminal.js`: eight modules,
xterm's stylesheet, the code font) and its server drew the first screen, or at the latest
`REVEAL_LIMIT_MS` (5 s, `terminal-core.js`) after `start()`, which comes after the core load. The
test's whole budget was shorter than the app's own limit, so a starved runner (#949) or a silent
server failed it at any load.

## Decision

- **`terminalShown` waits up to `TERMINAL_SHOW_MS` (30 s, `e2e/tests/support.ts`)**, with a
  failure message saying the terminal never showed. It bounds that one condition, not the steps
  after it, as `MONACO_LOAD_MS` does for Monaco.
- **The budget follows the app's contract**: it covers `REVEAL_LIMIT_MS` plus the terminal core's
  load on a starved runner. A change to `REVEAL_LIMIT_MS` or to what the core loads revisits it.
- **A regression test drives the app's fallback** (`terminal.e2e.ts`, #949): the core is held until
  the wait has begun and the hello is never answered, so the terminal shows only by the fallback,
  more than 5 s after the wait began.

## Alternatives

- **Keep 5 s and reveal sooner in the app.** Rejected: the fallback exists for a silent server and
  would cover a slow one with an empty terminal; a product change for a test.
- **Wait for the core load first, then 5 s.** Rejected: still no longer than the fallback, and a
  second condition to keep in step with `terminal.js`.
- **Retries.** Rejected: `retries: 0` stays (`e2e/e2e.config.ts`); a retry hides the wait instead of sizing it.

## Consequences

The 24 calls in `archive`, `reply`, `terminal`, `terminals` and `watching` no longer fail
for a slow terminal start alone; a terminal that never shows still fails, after 30 s instead of
5 s, with a message that names it.
